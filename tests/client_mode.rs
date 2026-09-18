//! Integration tests for thin client mode.

#![cfg(unix)]

pub mod support;
#[path = "support/terminal_screen.rs"]
mod terminal_screen;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde_json::Value;
use support::{
    cleanup_test_base, client_shell_handshake, read_server_message, register_runtime_dir,
    register_spawned_herdr_pid, unregister_spawned_herdr_pid, wait_for_client_shell_bootstrap,
    wait_for_message_variant, wait_for_message_variants, wait_for_socket, wait_until,
    CURRENT_ENDPOINT_PROTOCOL_GENERATION as CURRENT_PROTOCOL, SERVER_MESSAGE_PANE_SURFACE,
    SERVER_MESSAGE_PANE_SURFACE_PATCH, SERVER_MESSAGE_SEMANTIC_NOTIFICATION,
    SERVER_MESSAGE_SERVER_SHUTDOWN,
};

fn unique_test_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    PathBuf::from(format!(
        "/tmp/herdr-client-test-{}-{nanos}",
        std::process::id()
    ))
}

struct SpawnedHerdr {
    _master: Option<Box<dyn MasterPty + Send>>,
    child: Box<dyn Child + Send + Sync>,
}

impl SpawnedHerdr {
    fn close_master(&mut self) {
        drop(self._master.take());
    }
}

impl Drop for SpawnedHerdr {
    fn drop(&mut self) {
        let pid = self.child.process_id();
        let _ = self.child.kill();
        self.close_master();

        if let Some(pid) = pid {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                let mut status = 0;
                let result =
                    unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
                if result == pid as libc::pid_t || result == -1 {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }

            unregister_spawned_herdr_pid(Some(pid));
        }
    }
}

fn cleanup_spawned_herdr(spawned: SpawnedHerdr, base: PathBuf) {
    drop(spawned);
    cleanup_test_base(&base);
}

struct TestBaseCleanup(PathBuf);

impl Drop for TestBaseCleanup {
    fn drop(&mut self) {
        cleanup_test_base(&self.0);
    }
}

fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn spawn_client_process(
    config_home: &PathBuf,
    runtime_dir: &PathBuf,
    api_socket_path: &PathBuf,
) -> SpawnedHerdr {
    spawn_client_process_with_args(config_home, runtime_dir, api_socket_path, &["client"])
}

fn spawn_client_shell_process(
    config_home: &PathBuf,
    runtime_dir: &PathBuf,
    api_socket_path: &PathBuf,
) -> SpawnedHerdr {
    spawn_client_process_with_args(config_home, runtime_dir, api_socket_path, &["client"])
}

fn spawn_client_process_with_args(
    config_home: &PathBuf,
    runtime_dir: &PathBuf,
    api_socket_path: &PathBuf,
    args: &[&str],
) -> SpawnedHerdr {
    spawn_client_process_with_args_and_env(config_home, runtime_dir, api_socket_path, args, &[])
}

fn spawn_client_process_with_args_and_env(
    config_home: &PathBuf,
    runtime_dir: &PathBuf,
    api_socket_path: &PathBuf,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> SpawnedHerdr {
    register_runtime_dir(runtime_dir);
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_herdr"));
    cmd.args(args);
    cmd.env("HERDR_DISABLE_SOUND", "1");
    cmd.env("XDG_STATE_HOME", runtime_dir.join("state"));
    cmd.env("XDG_CONFIG_HOME", config_home);
    cmd.env("XDG_RUNTIME_DIR", runtime_dir);
    cmd.env("HERDR_SOCKET_PATH", api_socket_path);
    cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
    cmd.env("SHELL", "/bin/sh");
    cmd.env_remove("HERDR_ENV");
    for (key, value) in extra_env {
        cmd.env(key, value);
    }

    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_herdr_pid(child.process_id());
    drop(pair.slave);

    SpawnedHerdr {
        _master: Some(pair.master),
        child,
    }
}

fn spawn_server(
    config_home: &PathBuf,
    runtime_dir: &PathBuf,
    api_socket_path: &PathBuf,
    client_socket_path: &PathBuf,
) -> SpawnedHerdr {
    spawn_server_with_config(
        config_home,
        runtime_dir,
        api_socket_path,
        client_socket_path,
        "onboarding = false\n",
    )
}

fn spawn_server_with_config(
    config_home: &PathBuf,
    runtime_dir: &PathBuf,
    api_socket_path: &PathBuf,
    _client_socket_path: &PathBuf,
    config: &str,
) -> SpawnedHerdr {
    fs::create_dir_all(config_home.join(app_dir_name())).unwrap();
    fs::create_dir_all(runtime_dir).unwrap();
    register_runtime_dir(runtime_dir);
    fs::write(config_home.join(app_dir_name()).join("config.toml"), config).unwrap();

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_herdr"));
    cmd.arg("server");
    cmd.env("XDG_CONFIG_HOME", config_home);
    cmd.env("XDG_RUNTIME_DIR", runtime_dir);
    cmd.env("HERDR_SOCKET_PATH", api_socket_path);
    cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
    cmd.env("SHELL", "/bin/sh");
    cmd.env_remove("HERDR_ENV");

    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_herdr_pid(child.process_id());
    drop(pair.slave);

    SpawnedHerdr {
        _master: Some(pair.master),
        child,
    }
}

fn ping_socket(socket_path: &PathBuf) -> String {
    let mut stream = UnixStream::connect(socket_path).expect("should connect to API socket");

    let request = r#"{"id":"1","method":"ping","params":{}}"#;
    writeln!(stream, "{}", request).unwrap();

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response).unwrap();
    response.trim().to_string()
}

fn send_json_request(socket_path: &PathBuf, request: &str) -> Value {
    let mut stream = UnixStream::connect(socket_path).expect("should connect to API socket");
    writeln!(stream, "{}", request).unwrap();

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response).unwrap();
    serde_json::from_str(&response).expect("response should be valid JSON")
}

fn first_pane_id_in_workspace(socket_path: &PathBuf, workspace_id: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let request = format!(
            r#"{{"id":"pane_list","method":"pane.list","params":{{"workspace_id":"{workspace_id}"}}}}"#
        );
        let panes = send_json_request(socket_path, &request);
        if let Some(pane_id) = panes["result"]["panes"]
            .as_array()
            .and_then(|panes| panes.first())
            .and_then(|pane| pane["pane_id"].as_str())
        {
            return pane_id.to_string();
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("pane.list did not return a pane for workspace {workspace_id} before timeout");
}

fn app_dir_name() -> &'static str {
    if cfg!(debug_assertions) {
        "herdr-dev"
    } else {
        "herdr"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn client_connects_and_receives_pane_surface() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let spawned = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let mut stream = UnixStream::connect(&client_socket).expect("should connect to client socket");
    let (version, error) = client_shell_handshake(&mut stream, CURRENT_PROTOCOL, 54, 23)
        .expect("handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "{error:?}");
    wait_for_client_shell_bootstrap(&mut stream, Duration::from_secs(10))
        .expect("should receive the shell snapshot and pane surface");

    cleanup_spawned_herdr(spawned, base);
}

#[test]
fn direct_attach_initial_mouse_capture_follows_config() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");
    let config_path = config_home.join(app_dir_name()).join("config.toml");

    let spawned_server = spawn_server_with_config(
        &config_home,
        &runtime_dir,
        &api_socket,
        &client_socket,
        "onboarding = false\n[ui]\nmouse_capture = false\n",
    );
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));
    let created = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "create-workspace-for-direct-attach",
            "method": "workspace.create",
            "params": {"cwd": base},
        })
        .to_string(),
    );
    let terminal_id = created["result"]["root_pane"]["terminal_id"]
        .as_str()
        .expect("created terminal id")
        .to_string();

    let mut attach = spawn_client_process_with_args(
        &config_home,
        &runtime_dir,
        &api_socket,
        &["terminal", "attach", &terminal_id],
    );
    let output = spawn_pty_drain(
        attach
            ._master
            .as_ref()
            .expect("direct attach master")
            .try_clone_reader()
            .expect("clone direct attach PTY reader"),
    );
    assert!(
        wait_until(Duration::from_secs(5), Duration::from_millis(20), || {
            read_output(&output).contains("\x1b[?7l")
        }),
        "direct attach terminal setup should complete; output: {:?}",
        read_output(&output)
    );
    assert!(
        !read_output(&output).contains("\x1b[?1000h"),
        "mouse capture disabled must not enable host mouse reporting; output: {:?}",
        read_output(&output)
    );
    assert!(
        read_output(&output).contains("\x1b[?2004h"),
        "direct attach must enable host bracketed paste; output: {:?}",
        read_output(&output)
    );

    let restore_watermark = output_len(&output);
    attach
        ._master
        .as_ref()
        .expect("direct attach master")
        .take_writer()
        .expect("direct attach PTY writer")
        .write_all(b"\x02q")
        .expect("detach direct attach client");
    let restore_output = drain_until_client_exits(&mut attach, &output, restore_watermark);
    assert!(
        restore_output.contains("\x1b[?2004l"),
        "direct attach must disable host bracketed paste on restore; output: {restore_output:?}"
    );
    drop(attach);

    fs::write(
        &config_path,
        "onboarding = false\n[ui]\nmouse_capture = true\n",
    )
    .unwrap();
    let attach = spawn_client_process_with_args(
        &config_home,
        &runtime_dir,
        &api_socket,
        &["terminal", "attach", &terminal_id],
    );
    let output = spawn_pty_drain(
        attach
            ._master
            .as_ref()
            .expect("direct attach master")
            .try_clone_reader()
            .expect("clone direct attach PTY reader"),
    );
    assert!(
        wait_until(Duration::from_secs(5), Duration::from_millis(20), || {
            read_output(&output).contains("\x1b[?7l")
        }),
        "direct attach terminal setup should complete; output: {:?}",
        read_output(&output)
    );
    assert!(
        read_output(&output).contains("\x1b[?1000h"),
        "mouse capture enabled must retain host mouse reporting; output: {:?}",
        read_output(&output)
    );

    drop(spawned_server);
    cleanup_spawned_herdr(attach, base);
}

#[test]
fn client_sees_headless_startup_config_diagnostic() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let app_dir = if cfg!(debug_assertions) {
        "herdr-dev"
    } else {
        "herdr"
    };
    fs::create_dir_all(config_home.join(app_dir)).unwrap();
    fs::write(
        config_home.join(app_dir).join("config.toml"),
        "[keys\nprefix = \"ctrl+a\"\n",
    )
    .unwrap();
    fs::create_dir_all(&runtime_dir).unwrap();
    register_runtime_dir(&runtime_dir);

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_herdr"));
    cmd.arg("server");
    cmd.env("XDG_CONFIG_HOME", &config_home);
    cmd.env("XDG_RUNTIME_DIR", &runtime_dir);
    cmd.env("HERDR_SOCKET_PATH", &api_socket);
    cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
    cmd.env("SHELL", "/bin/sh");
    cmd.env_remove("HERDR_ENV");

    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_herdr_pid(child.process_id());
    drop(pair.slave);

    let spawned = SpawnedHerdr {
        _master: Some(pair.master),
        child,
    };
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let client = spawn_client_shell_process(&config_home, &runtime_dir, &api_socket);
    let output = spawn_pty_drain(
        client
            ._master
            .as_ref()
            .expect("client shell master")
            .try_clone_reader()
            .expect("clone client shell reader"),
    );
    assert!(
        wait_until(Duration::from_secs(8), Duration::from_millis(20), || {
            let output = read_output(&output);
            output.contains("config.toml") && output.contains("herdr config check")
        }),
        "client shell should render startup config diagnostic; output: {:?}",
        read_output(&output)
    );

    drop(spawned);
    cleanup_spawned_herdr(client, base);
}

#[test]
fn server_unreachable_shows_clear_error() {
    // when server is unreachable, the client exits quickly
    // with an actionable connection-failed message.
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");

    fs::create_dir_all(config_home.join("herdr")).unwrap();
    fs::create_dir_all(&runtime_dir).unwrap();
    register_runtime_dir(&runtime_dir);
    fs::write(
        config_home.join("herdr/config.toml"),
        "onboarding = false\n",
    )
    .unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_herdr"))
        .arg("client")
        .env("HERDR_DISABLE_SOUND", "1")
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("XDG_STATE_HOME", runtime_dir.join("state"))
        .env("HERDR_SOCKET_PATH", &api_socket)
        .env_remove("HERDR_CLIENT_SOCKET_PATH")
        .env_remove("HERDR_ENV")
        .output()
        .expect("client command should run");

    assert!(
        !output.status.success(),
        "client should fail when no server is running"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to connect to server"),
        "stderr should mention connection failure: {stderr}"
    );
    assert!(
        stderr.contains("Is herdr server running?"),
        "stderr should include actionable guidance: {stderr}"
    );
    assert!(
        stderr.contains("Socket path:"),
        "stderr should include attempted socket path: {stderr}"
    );

    cleanup_test_base(&base);
}

#[test]
fn server_crash_after_attach_causes_lost_connection_error() {
    // attach a real thin client connection, kill server unexpectedly,
    // assert clean non-zero client exit plus lost-connection signal.
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let mut spawned = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    // Attach a real thin client (client subcommand) through PTY so handshake and
    // terminal setup paths are exercised.
    let mut thin_client = spawn_client_process(&config_home, &runtime_dir, &api_socket);

    // Prove attached before kill by waiting for recognizable rendered app content.
    let mut thin_reader = thin_client
        ._master
        .as_ref()
        .expect("thin client master")
        .try_clone_reader()
        .expect("clone client PTY reader");
    let (attached_before_kill, attach_output) = {
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut buf = [0u8; 4096];
        let mut seen = false;
        let mut output = String::new();
        while Instant::now() < deadline {
            match thin_reader.read(&mut buf) {
                Ok(n) if n > 0 => {
                    let out = String::from_utf8_lossy(&buf[..n]);
                    output.push_str(&out);
                    if out.contains("\u{2500}")
                        || out.contains("workspace")
                        || out.contains("pane")
                        || out.contains("terminal")
                    {
                        seen = true;
                        break;
                    }
                    if output.to_lowercase().contains("herdr:") {
                        break;
                    }
                }
                Ok(_) => thread::sleep(Duration::from_millis(30)),
                Err(_) => thread::sleep(Duration::from_millis(30)),
            }
        }
        (seen, output)
    };
    assert!(
        attached_before_kill,
        "thin client must complete attach and receive frame before server crash; output: {attach_output:?}"
    );

    // Kill server unexpectedly.
    if let Some(pid) = spawned.child.process_id() {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    spawned.close_master();

    // Client should exit non-zero after connection loss.
    let mut crash_output = String::new();
    let exited = {
        let deadline = Instant::now() + Duration::from_secs(12);
        let mut exited = false;
        while Instant::now() < deadline {
            if thin_client.child.try_wait().ok().flatten().is_some() {
                exited = true;
                break;
            }
            // Keep draining client output so the process can progress to exit.
            let mut buf = [0u8; 1024];
            if let Ok(n) = thin_reader.read(&mut buf) {
                if n > 0 {
                    crash_output.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        exited
    };
    assert!(exited, "thin client should exit after server SIGKILL");

    let status = thin_client.child.wait().expect("wait thin client status");
    assert!(
        !status.success(),
        "thin client should exit non-zero after lost server connection"
    );

    // Drain trailing output and require the explicit user-visible lost-connection message.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        match thin_reader.read(&mut buf) {
            Ok(n) if n > 0 => crash_output.push_str(&String::from_utf8_lossy(&buf[..n])),
            Ok(_) => break,
            Err(_) => break,
        }
        thread::sleep(Duration::from_millis(30));
    }

    let crash_output_lc = crash_output.to_lowercase();
    assert!(
        crash_output_lc.contains("lost connection to server"),
        "thin client must emit explicit lost-connection message after server crash; output: {crash_output:?}"
    );

    // Ensure server is gone.
    let _ = spawned.child.wait();

    cleanup_test_base(&base);
}

/// Any of the mouse-disable modes emitted by `clear_host_mouse_reporting` on
/// terminal restore. Their presence in the client's PTY output proves the
/// restore path (`TerminalGuard::Drop` → `restore_terminal_state`) ran.
const MOUSE_TEARDOWN_MARKERS: [&str; 2] = ["\u{1b}[?1003l", "\u{1b}[?1000l"];

fn output_has_mouse_teardown(output: &str) -> bool {
    MOUSE_TEARDOWN_MARKERS
        .iter()
        .all(|marker| output.contains(marker))
}

/// Shared buffer fed by a background PTY reader thread. Reading on a thread
/// keeps the blocking `Box<dyn Read>` (which has no timeout) off the test's
/// main thread, so a client that never exits fails the deadline instead of
/// hanging the whole test forever.
#[derive(Default)]
struct PtyOutput {
    bytes: Vec<u8>,
    // Keep legacy raw-string watermarks stable even across split UTF-8 reads.
    text: String,
}

type SharedOutput = std::sync::Arc<Mutex<PtyOutput>>;

fn spawn_pty_drain(mut reader: Box<dyn Read + Send>) -> SharedOutput {
    let output: SharedOutput = std::sync::Arc::new(Mutex::new(PtyOutput::default()));
    let thread_output = output.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let mut captured = thread_output.lock().unwrap_or_else(|p| p.into_inner());
                    captured.bytes.extend_from_slice(&buf[..n]);
                    captured.text.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
                Err(_) => break,
            }
        }
    });
    output
}

#[test]
fn screen_capture_preserves_split_utf8_bytes() {
    let reader = std::io::Cursor::new(b"caf\xc3").chain(std::io::Cursor::new(b"\xa9"));
    let output = spawn_pty_drain(Box::new(reader));
    assert!(wait_until(
        Duration::from_secs(2),
        Duration::from_millis(10),
        || {
            let bytes = output.lock().unwrap().bytes.clone();
            bytes == "café".as_bytes() && terminal_screen::text(&bytes, 80, 24).contains("café")
        }
    ));
}

#[test]
fn pane_gap_background_visual_ownership() {
    verify_pane_background_ownership(false);
}

#[test]
fn pane_sidebar_gap_matches_split_gutter() {
    verify_pane_background_ownership(true);
}

const PANE_BACKGROUND_CONFIG: &str = r##"onboarding = false

[ui]
sidebar_start_collapsed = true
sidebar_collapsed_mode = "hidden"
sidebar_width = 20
hide_tab_bar_when_single_tab = true
pane_borders = "auto"
pane_outer_borders = true
pane_scrollbars = true
pane_gaps = true
mouse_capture = false

[theme]
name = "terminal"

[theme.custom]
accent = "#a9dc76"
overlay0 = "#5b595c"
sidebar_bg = "#181825"
pane_gap_bg = "#221f22"
pane_default_bg = "#1e1e2e"
"##;

#[test]
fn line_tabs_keep_panel_background_and_follow_focus() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let cleanup = TestBaseCleanup(base.clone());
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");
    let config = PANE_BACKGROUND_CONFIG
        .replace(
            "hide_tab_bar_when_single_tab = true",
            "hide_tab_bar_when_single_tab = false",
        )
        .replace(
            "mouse_capture = false",
            "mouse_capture = true\nmobile_width_threshold = 0",
        )
        .replace(
            "sidebar_bg = \"#181825\"",
            "sidebar_bg = \"#181825\"\npanel_bg = \"#221f22\"",
        );
    let server = spawn_server_with_config(
        &config_home,
        &runtime_dir,
        &api_socket,
        &client_socket,
        &config,
    );
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let created = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "line-tabs-workspace",
            "method": "workspace.create",
            "params": {"cwd": &base, "focus": true},
        })
        .to_string(),
    );
    let workspace_id = created["result"]["workspace"]["workspace_id"]
        .as_str()
        .expect("workspace id")
        .to_string();
    let first_tab = created["result"]["tab"]["tab_id"]
        .as_str()
        .expect("first tab id")
        .to_string();
    let first_pane = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("first pane id")
        .to_string();
    let mut tabs = vec![(first_tab, first_pane, "First")];
    for (id, label) in [("line-tabs-second", "Second"), ("line-tabs-third", "Third")] {
        let response = send_json_request(
            &api_socket,
            &serde_json::json!({
                "id": id,
                "method": "tab.create",
                "params": {"workspace_id": &workspace_id, "focus": false},
            })
            .to_string(),
        );
        tabs.push((
            response["result"]["tab"]["tab_id"]
                .as_str()
                .expect("tab id")
                .to_string(),
            response["result"]["root_pane"]["pane_id"]
                .as_str()
                .expect("pane id")
                .to_string(),
            label,
        ));
    }
    for (tab_id, _, label) in &tabs {
        let response = send_json_request(
            &api_socket,
            &serde_json::json!({
                "id": format!("rename-{label}"),
                "method": "tab.rename",
                "params": {"tab_id": tab_id, "label": label},
            })
            .to_string(),
        );
        assert!(response.get("result").is_some(), "{response}");
    }
    send_pane_shell_command(&api_socket, &tabs[0].1, "printf 'FIRST_READY\\n'");

    let (mut client, output) = attach_pixel_client(&config_home, &runtime_dir, &api_socket);
    let (baseline_bytes, baseline_screen) = wait_for_tab_frame(
        &output,
        80,
        24,
        &["First", "Second", "Third", "FIRST_READY"],
    );
    write_tab_evidence("", &baseline_bytes, &baseline_screen, &config);
    let mut replay = baseline_bytes.clone();
    assert_tab_label_colors(
        &baseline_bytes,
        &baseline_screen,
        80,
        &[("First", [169, 220, 118]), ("Second", [91, 89, 92])],
    );
    assert_tab_label_backgrounds(
        &baseline_bytes,
        &baseline_screen,
        80,
        &["First", "Second", "Third"],
    );
    assert_pixel_tab_underline(&baseline_bytes);
    assert_pixel_tab_underline_placement(&baseline_bytes, &baseline_screen, "First");

    let (narrow_bytes, narrow_screen) = resize_and_wait_for_tab_frame(
        &mut client,
        &output,
        22,
        &["First", "FIRST_READY", "<", ">", "+"],
    );
    assert_tab_label_colors(
        &narrow_bytes,
        &narrow_screen,
        22,
        &[("First", [169, 220, 118])],
    );
    assert_pixel_tab_underline_placement(&narrow_bytes, &narrow_screen, "First");
    write_tab_evidence("narrow", &narrow_bytes, &narrow_screen, &config);
    replay.extend_from_slice(&narrow_bytes);

    output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .bytes
        .clear();
    let focused = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "focus-second-line-tab",
            "method": "tab.focus",
            "params": {"tab_id": &tabs[1].0},
        })
        .to_string(),
    );
    assert_eq!(focused["result"]["tab"]["focused"], true, "{focused}");
    send_pane_shell_command(&api_socket, &tabs[1].1, "printf 'SECOND_READY\\n'");
    let (focused_bytes, focused_screen) =
        wait_for_tab_frame(&output, 22, 24, &["Second", "SECOND_READY"]);
    assert_tab_label_colors(
        &focused_bytes,
        &focused_screen,
        22,
        &[("Second", [169, 220, 118])],
    );
    assert_pixel_tab_underline_placement(&focused_bytes, &focused_screen, "Second");
    write_tab_evidence("focused-second", &focused_bytes, &focused_screen, &config);
    replay.extend_from_slice(&focused_bytes);

    let (wide_bytes, wide_screen) = resize_and_wait_for_tab_frame(
        &mut client,
        &output,
        85,
        &["First", "Second", "Third", "SECOND_READY"],
    );
    assert_tab_label_colors(
        &wide_bytes,
        &wide_screen,
        85,
        &[("First", [91, 89, 92]), ("Second", [169, 220, 118])],
    );
    assert_tab_label_backgrounds(&wide_bytes, &wide_screen, 85, &["First", "Second", "Third"]);
    assert_pixel_tab_underline_placement(&wide_bytes, &wide_screen, "Second");
    write_tab_evidence("wide", &wide_bytes, &wide_screen, &config);
    replay.extend_from_slice(&wide_bytes);
    write_tab_evidence("replay", &replay, &wide_screen, &config);

    drop(client);
    drop(server);
    drop(cleanup);
}

fn resize_and_wait_for_tab_frame(
    client: &mut SpawnedHerdr,
    output: &SharedOutput,
    cols: u16,
    markers: &[&str],
) -> (Vec<u8>, String) {
    output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .bytes
        .clear();
    client
        ._master
        .as_ref()
        .expect("client PTY")
        .resize(PtySize {
            rows: 24,
            cols,
            pixel_width: cols * 17,
            pixel_height: 24 * 36,
        })
        .expect("resize client PTY");
    wait_for_tab_frame(output, cols, 24, markers)
}

fn wait_for_tab_frame(
    output: &SharedOutput,
    cols: u16,
    rows: u16,
    markers: &[&str],
) -> (Vec<u8>, String) {
    assert!(
        wait_until(Duration::from_secs(8), Duration::from_millis(20), || {
            let bytes = output
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .bytes
                .clone();
            let completed = completed_client_frame(&bytes);
            if completed.is_empty() {
                return false;
            }
            let screen = terminal_screen::text(completed, cols, rows);
            markers.iter().all(|marker| screen.contains(marker))
        }),
        "tab frame did not contain {markers:?}: {:?}",
        read_output(output)
    );
    let bytes = output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .bytes
        .clone();
    let bytes = completed_client_frame(&bytes).to_vec();
    let screen = terminal_screen::text(&bytes, cols, rows);
    (bytes, screen)
}

fn tab_label_position(screen: &str, label: &str) -> (usize, usize) {
    screen
        .lines()
        .enumerate()
        .find_map(|(row, line)| {
            line.find(label).map(|byte_column| {
                (
                    row,
                    unicode_width::UnicodeWidthStr::width(&line[..byte_column]),
                )
            })
        })
        .unwrap_or_else(|| panic!("missing tab label {label:?} in {screen:?}"))
}

fn assert_tab_label_colors(bytes: &[u8], screen: &str, cols: u16, expected: &[(&str, [u8; 3])]) {
    let foregrounds = terminal_screen::explicit_foregrounds(bytes, cols, 24);
    for (label, color) in expected {
        let (row, column) = tab_label_position(screen, label);
        for x in column..column + label.len() {
            assert_eq!(
                foregrounds[row * usize::from(cols) + x],
                Some(*color),
                "label {label:?} at {column},{row}, cell {x}: {screen:?}"
            );
        }
    }
}

fn tab_underline_png(bytes: &[u8]) -> Option<Vec<u8>> {
    use base64::Engine;
    let packets = regex::bytes::Regex::new(r"\x1b_G([^;]*f=100[^;]*);([^\x1b]*)\x1b\\").unwrap();
    let underline = packets.captures_iter(bytes).find_map(|packet| {
        let encoded = base64::engine::general_purpose::STANDARD
            .decode(&packet[2])
            .unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(&encoded))
            .read_info()
            .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut pixels).unwrap();
        (info.color_type == png::ColorType::Rgba).then_some(encoded)
    });
    underline
}

fn assert_pixel_tab_underline(bytes: &[u8]) {
    let png = tab_underline_png(bytes).expect("active tab underline PNG");
    let mut reader = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .unwrap();
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert_eq!(info.height, 36);
    let width = info.width as usize;
    assert_eq!(pixels[..width * 34 * 4], vec![0; width * 34 * 4]);
    let accent = if cfg!(target_os = "macos") {
        [179, 219, 130, 255]
    } else {
        [169, 220, 118, 255]
    };
    assert_eq!(pixels[width * 34 * 4..], accent.repeat(width * 2));
}

fn assert_pixel_tab_underline_placement(bytes: &[u8], screen: &str, label: &str) {
    let placement =
        regex::bytes::Regex::new(r"\x1b\[(\d+);(\d+)H\x1b_Ga=p,[^\x1b]*c=(\d+),r=1,z=-1").unwrap();
    let (label_row, label_column) = tab_label_position(screen, label);
    let placed = placement.captures_iter(bytes).any(|capture| {
        let row = std::str::from_utf8(&capture[1])
            .unwrap()
            .parse::<usize>()
            .unwrap()
            - 1;
        let column = std::str::from_utf8(&capture[2])
            .unwrap()
            .parse::<usize>()
            .unwrap()
            - 1;
        let columns = std::str::from_utf8(&capture[3])
            .unwrap()
            .parse::<usize>()
            .unwrap();
        row == label_row && column <= label_column && column + columns >= label_column + label.len()
    });
    assert!(placed, "pixel underline must span active label {label:?}");
}

fn assert_tab_label_backgrounds(bytes: &[u8], screen: &str, cols: u16, labels: &[&str]) {
    let backgrounds = terminal_screen::explicit_backgrounds(bytes, cols, 24);
    for label in labels {
        let (row, column) = tab_label_position(screen, label);
        for x in column..column + label.len() {
            assert_eq!(backgrounds[row * usize::from(cols) + x], Some([34, 31, 34]));
        }
    }
}

fn write_tab_evidence(name: &str, bytes: &[u8], screen: &str, config: &str) {
    let Some(root) = std::env::var_os("HERDR_VISUAL_EVIDENCE_DIR") else {
        return;
    };
    let mut path = PathBuf::from(root).join("tabs");
    if !name.is_empty() {
        path.push(name);
    }
    fs::create_dir_all(&path).expect("create tab evidence directory");
    fs::write(path.join("raw.ansi"), bytes).expect("write tab ANSI evidence");
    if let Some(png) = tab_underline_png(bytes) {
        fs::write(path.join("underline.png"), png).expect("write tab underline evidence");
    }
    fs::write(path.join("screen.txt"), screen).expect("write tab screen evidence");
    fs::write(path.join("config.toml"), config).expect("write tab config evidence");
}

fn verify_pane_background_ownership(show_sidebar: bool) {
    let _lock = test_lock();
    let base = unique_test_dir();
    let cleanup = TestBaseCleanup(base.clone());
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");
    let mut config = PANE_BACKGROUND_CONFIG.replace(
        "sidebar_start_collapsed = true",
        &format!("sidebar_start_collapsed = {}", !show_sidebar),
    );
    if show_sidebar {
        config = config.replace("mouse_capture = false", "mouse_capture = true");
    }
    let evidence_dir = std::env::var_os("HERDR_VISUAL_EVIDENCE_DIR").map(|path| {
        let path = PathBuf::from(path);
        if show_sidebar {
            path.join("sidebar")
        } else {
            path
        }
    });
    let server = spawn_server_with_config(
        &config_home,
        &runtime_dir,
        &api_socket,
        &client_socket,
        &config,
    );
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let created = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "pane-gap-workspace",
            "method": "workspace.create",
            "params": {"cwd": &base, "focus": true, "label": "pane-gap-proof"},
        })
        .to_string(),
    );
    let left_pane = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("left pane id");
    let right = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "pane-gap-split-right",
            "method": "pane.split",
            "params": {
                "target_pane_id": left_pane,
                "direction": "right",
                "ratio": 0.5,
                "focus": true,
            },
        })
        .to_string(),
    );
    let right_pane = right["result"]["pane"]["pane_id"]
        .as_str()
        .expect("right pane id");
    let bottom = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "pane-gap-split-down",
            "method": "pane.split",
            "params": {
                "target_pane_id": right_pane,
                "direction": "down",
                "ratio": 0.5,
                "focus": true,
            },
        })
        .to_string(),
    );
    let bottom_pane = bottom["result"]["pane"]["pane_id"]
        .as_str()
        .expect("bottom pane id");

    send_pane_shell_command(
        &api_socket,
        left_pane,
        "i=0; while [ \"$i\" -lt 30 ]; do printf 'history\\n'; i=$((i + 1)); done; printf '\\033[48;2;18;52;86mEXPLICIT_BG\\033[0m\\nLEFT_READY\\n'",
    );
    send_pane_shell_command(&api_socket, right_pane, "printf 'TOP_RIGHT_READY\\n'");
    send_pane_shell_command(&api_socket, bottom_pane, "printf 'BOTTOM_RIGHT_READY\\n'");
    let (client, output) = attach_pixel_client(&config_home, &runtime_dir, &api_socket);
    assert!(
        wait_until(Duration::from_secs(8), Duration::from_millis(20), || {
            let bytes = output
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .bytes
                .clone();
            let screen = terminal_screen::text(completed_client_frame(&bytes), 80, 24);
            ["LEFT_READY", "TOP_RIGHT_READY", "BOTTOM_RIGHT_READY"]
                .iter()
                .all(|marker| screen.contains(marker))
                && screen
                    .lines()
                    .nth(22)
                    .and_then(|line| line.chars().nth(if show_sidebar { 22 } else { 0 }))
                    == Some('▏')
        }),
        "three-pane client shell did not render: {:?}",
        read_output(&output)
    );

    let bytes = output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .bytes
        .clone();
    let bytes = completed_client_frame(&bytes).to_vec();
    let screen = terminal_screen::text(&bytes, 80, 24);
    if let Some(evidence_dir) = &evidence_dir {
        fs::create_dir_all(evidence_dir).expect("create visual evidence directory");
        fs::write(evidence_dir.join("raw.ansi"), &bytes).expect("write raw ANSI evidence");
        fs::write(evidence_dir.join("screen.txt"), &screen).expect("write screen evidence");
    }
    let backgrounds = terminal_screen::explicit_backgrounds(&bytes, 80, 24);
    let gap_background = Some([34, 31, 34]);
    let pane_default_background = Some([30, 30, 46]);
    let origin_x = if show_sidebar { 22 } else { 0 };
    let right_x = if show_sidebar { 51 } else { 40 };
    assert_eq!(
        backgrounds[80 + origin_x],
        pane_default_background,
        "the left edge border cell belongs to the pane, not the exterior"
    );
    assert_eq!(
        screen.lines().nth(1).unwrap().chars().nth(origin_x),
        Some('▏'),
        "the border stroke must sit on the cell's outside edge"
    );
    let application_background = Some([18, 52, 86]);
    let panes = if show_sidebar {
        [
            (22_u16, 0_u16, 28_u16, 24_u16),
            (51, 0, 29, 12),
            (51, 12, 29, 12),
        ]
    } else {
        [(0, 0, 39, 24), (40, 0, 40, 12), (40, 12, 40, 12)]
    };
    let mut pane_interior = 0;
    let mut pane_border = 0;
    let mut outside = 0;
    let mut pane_default_interior = 0;
    let mut pane_default_border = 0;
    let foregrounds = terminal_screen::explicit_foregrounds(&bytes, 80, 24);
    let symbols: Vec<Vec<char>> = screen.lines().map(|line| line.chars().collect()).collect();
    let mut application_interior = 0;
    let mut gap_outside = 0;
    for y in 0..24_u16 {
        if show_sidebar {
            if (1..23).contains(&y) {
                assert_eq!(symbols[usize::from(y)][20], '▕', "sidebar frame edge");
            }
            assert_eq!(backgrounds[usize::from(y) * 80], gap_background);
            assert_eq!(backgrounds[usize::from(y) * 80 + 21], gap_background);
            for x in [21, 50] {
                assert_eq!(
                    symbols
                        .get(usize::from(y))
                        .and_then(|row| row.get(x))
                        .copied()
                        .unwrap_or(' '),
                    ' ',
                    "one blank column at sidebar and split"
                );
                assert_eq!(backgrounds[usize::from(y) * 80 + x], gap_background);
            }
        }
        for x in origin_x as u16..80_u16 {
            let background = backgrounds[usize::from(y) * 80 + usize::from(x)];
            let owner = panes.iter().find(|(px, py, width, height)| {
                x >= *px && x < px + width && y >= *py && y < py + height
            });
            match owner {
                Some((px, py, width, height))
                    if x == *px || x + 1 == px + width || y == *py || y + 1 == py + height =>
                {
                    pane_border += 1;
                    pane_default_border += usize::from(background == pane_default_background);
                    let expected_symbol = match (
                        x == *px,
                        x + 1 == px + width,
                        y == *py,
                        y + 1 == py + height,
                    ) {
                        (_, _, true, _) | (_, _, _, true) => ' ',
                        (true, _, _, _) => '▏',
                        (_, true, _, _) => '▕',
                        _ => unreachable!(),
                    };
                    assert_eq!(
                        symbols
                            .get(usize::from(y))
                            .and_then(|row| row.get(usize::from(x)))
                            .copied()
                            .unwrap_or(' '),
                        expected_symbol,
                        "border glyph {x},{y}"
                    );
                    let expected_foreground = if usize::from(*px) == right_x && *py == 12 {
                        [169, 220, 118]
                    } else {
                        [91, 89, 92]
                    };
                    assert_eq!(
                        foregrounds[usize::from(y) * 80 + usize::from(x)],
                        Some(expected_foreground),
                        "border focus color {x},{y}"
                    );
                }
                Some(_) => {
                    pane_interior += 1;
                    pane_default_interior += usize::from(background == pane_default_background);
                    application_interior += usize::from(background == application_background);
                }
                None => {
                    outside += 1;
                    gap_outside += usize::from(background == gap_background);
                }
            }
        }
    }
    let census = format!(
        "pane background ownership census\ninside: {pane_interior} cells, pane default: {pane_default_interior}, application: {application_interior}\nborder: {pane_border} cells, pane default: {pane_default_border}\noutside: {outside} cells, gap background: {gap_outside}\n"
    );
    eprint!("{census}");
    if let Some(evidence_dir) = &evidence_dir {
        fs::write(evidence_dir.join("config.toml"), &config).expect("write config evidence");
        fs::write(evidence_dir.join("census.txt"), &census).expect("write census evidence");
    }

    assert_eq!(pane_interior, if show_sidebar { 1112 } else { 1574 });
    assert_eq!(pane_border, if show_sidebar { 256 } else { 322 });
    assert_eq!(outside, 24);
    assert_eq!(
        pane_default_interior + application_interior,
        pane_interior,
        "terminal interiors must use the pane default unless an application set the background"
    );
    assert_eq!(
        application_interior, 11,
        "the application truecolor background must remain visible"
    );
    assert_eq!(
        pane_default_border, pane_border,
        "edge-aligned border cells must belong to the pane background"
    );
    assert_eq!(
        gap_outside, outside,
        "every cell outside the pane frames must use the pane gap background"
    );

    if show_sidebar {
        let sidebar_background = Some([24, 24, 37]);
        for y in 0..24 {
            assert_eq!(backgrounds[y * 80], gap_background);
            assert_eq!(backgrounds[y * 80 + 21], gap_background);
            if (1..23).contains(&y) {
                assert_eq!(backgrounds[y * 80 + 1], sidebar_background);
                assert_eq!(backgrounds[y * 80 + 20], sidebar_background);
            }
        }
        assert_eq!(backgrounds[2 * 80 + 10], sidebar_background);
        assert_eq!(backgrounds[20 * 80 + 10], sidebar_background);
    }

    let sidebar_inside = if cfg!(target_os = "macos") {
        [24, 24, 36]
    } else {
        [24, 24, 37]
    };
    let sidebar_inside_specs = [Some(sidebar_inside), Some(sidebar_inside)];
    assert_pixel_border_images(
        &bytes,
        if show_sidebar {
            &[340, 340, 476, 476, 493, 493, 493, 493]
        } else {
            &[663, 663, 680, 680, 680, 680]
        },
        &[],
        if show_sidebar {
            &sidebar_inside_specs
        } else {
            &[]
        },
    );

    let streaming_start = output.lock().unwrap().bytes.len();
    let end_sync = b"\x1b[?2026l";
    for update in 1..=3 {
        send_pane_shell_command(
            &api_socket,
            right_pane,
            &format!("printf '\\033[2;2HSTREAM_%s' '{update}'"),
        );
        assert!(wait_until(
            Duration::from_secs(5),
            Duration::from_millis(10),
            || {
                let bytes = output.lock().unwrap().bytes.clone();
                bytes
                    .windows(end_sync.len())
                    .rposition(|bytes| bytes == end_sync)
                    .is_some_and(|end| {
                        terminal_screen::text(&bytes[..end + end_sync.len()], 80, 24)
                            .contains(&format!("STREAM_{update}"))
                    })
            }
        ));
    }
    let streaming_bytes = output.lock().unwrap().bytes.clone();
    assert!(
        !streaming_bytes[streaming_start..]
            .windows(5)
            .any(|bytes| bytes == b"a=t,t"),
        "text updates must reuse uploaded border images"
    );
    let mut streamed_frames = 0;
    for (index, _) in streaming_bytes
        .windows(end_sync.len())
        .enumerate()
        .filter(|(index, bytes)| *index >= streaming_start && *bytes == end_sync)
    {
        let frame_bytes = &streaming_bytes[..index + end_sync.len()];
        if !terminal_screen::text(frame_bytes, 80, 24).contains("STREAM_") {
            continue;
        }
        let backgrounds = terminal_screen::explicit_backgrounds(frame_bytes, 80, 24);
        for y in 1..11_usize {
            for x in right_x + 1..79_usize {
                assert_eq!(
                    backgrounds[y * 80 + x],
                    pane_default_background,
                    "streaming frame {streamed_frames}, pane body cell {x},{y}"
                );
            }
        }
        streamed_frames += 1;
    }
    assert!(streamed_frames >= 3, "observe each incremental update");
    eprintln!("streaming background ownership: {streamed_frames} frames passed");
    if let Some(evidence_dir) = &evidence_dir {
        fs::write(evidence_dir.join("streaming.ansi"), &streaming_bytes)
            .expect("write streaming evidence");
    }

    if show_sidebar {
        let update_start = output.lock().unwrap().bytes.len();
        send_pane_shell_command(&api_socket, right_pane, "printf '\\033[2;2HSIDEBAR_STREAM'");
        assert!(wait_until(
            Duration::from_secs(5),
            Duration::from_millis(20),
            || terminal_screen::text(
                completed_client_frame(&output.lock().unwrap().bytes),
                80,
                24,
            )
            .contains("SIDEBAR_STREAM")
        ));
        assert!(!output.lock().unwrap().bytes[update_start..]
            .windows(5)
            .any(|bytes| bytes == b"a=t,t"));

        let created = send_json_request(
            &api_socket,
            &serde_json::json!({
                "id": "sidebar-click-workspace",
                "method": "workspace.create",
                "params": {"cwd": &base, "focus": false, "label": "click-proof"},
            })
            .to_string(),
        );
        let click_pane = created["result"]["root_pane"]["pane_id"]
            .as_str()
            .expect("click workspace pane");
        send_pane_shell_command(&api_socket, click_pane, "printf 'SIDEBAR_CLICK_READY\\n'");
        assert!(wait_until(
            Duration::from_secs(5),
            Duration::from_millis(20),
            || terminal_screen::text(
                completed_client_frame(&output.lock().unwrap().bytes),
                80,
                24,
            )
            .contains("click-proof")
        ));
        let click_screen = terminal_screen::text(
            completed_client_frame(&output.lock().unwrap().bytes),
            80,
            24,
        );
        let mut input = client._master.as_ref().unwrap().take_writer().unwrap();
        input
            .write_all(&sidebar_row_click(&click_screen, "click-proof"))
            .expect("click workspace row");
        assert!(wait_until(
            Duration::from_secs(5),
            Duration::from_millis(20),
            || terminal_screen::text(
                completed_client_frame(&output.lock().unwrap().bytes),
                80,
                24,
            )
            .contains("SIDEBAR_CLICK_READY")
        ));
        let selected = terminal_screen::text(
            completed_client_frame(&output.lock().unwrap().bytes),
            80,
            24,
        );
        input
            .write_all(&sidebar_row_click(&selected, "pane-gap-pro"))
            .expect("restore pane workspace");
        assert!(wait_until(
            Duration::from_secs(5),
            Duration::from_millis(20),
            || terminal_screen::text(
                completed_client_frame(&output.lock().unwrap().bytes),
                80,
                24,
            )
            .contains("LEFT_READY")
        ));

        let collapse_start = output.lock().unwrap().bytes.len();
        input.write_all(b"\x02b").expect("collapse sidebar");
        assert!(wait_until(
            Duration::from_secs(5),
            Duration::from_millis(20),
            || {
                let captured = output.lock().unwrap();
                let frame = terminal_screen::text(completed_client_frame(&captured.bytes), 80, 24);
                !frame.contains("spaces")
                    && captured.bytes[collapse_start..]
                        .windows(3)
                        .any(|bytes| bytes == b"a=d")
            }
        ));
        input.write_all(b"\x02b").expect("reveal sidebar");
        assert!(wait_until(
            Duration::from_secs(5),
            Duration::from_millis(20),
            || {
                let captured = output.lock().unwrap();
                terminal_screen::text(completed_client_frame(&captured.bytes), 80, 24)
                    .contains("spaces")
            }
        ));

        input
            .write_all(b"\x1b[<0;21;6M\x1b[<32;25;6M\x1b[<0;25;6m")
            .expect("resize sidebar");
        assert!(wait_until(
            Duration::from_secs(5),
            Duration::from_millis(20),
            || {
                let captured = output.lock().unwrap();
                let frame = terminal_screen::text(completed_client_frame(&captured.bytes), 80, 24);
                sidebar_right_edge(&frame) == Some(24)
            }
        ));
        if let Some(evidence_dir) = &evidence_dir {
            fs::write(
                evidence_dir.join("interaction.ansi"),
                &output.lock().unwrap().bytes,
            )
            .expect("write sidebar interaction evidence");
        }
    }

    drop(client);
    drop(server);
    drop(cleanup);
}

#[test]
fn named_stacked_panes_emit_filled_title_styles_and_pixel_uploads() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let cleanup = TestBaseCleanup(base.clone());
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");
    let config = PANE_BACKGROUND_CONFIG;
    let server = spawn_server_with_config(
        &config_home,
        &runtime_dir,
        &api_socket,
        &client_socket,
        config,
    );
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));
    let created = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "named-pane-workspace",
            "method": "workspace.create",
            "params": {"cwd": &base, "focus": true, "label": "named-pane-proof"},
        })
        .to_string(),
    );
    let top_pane = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("top pane id");
    let split = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "named-pane-split",
            "method": "pane.split",
            "params": {
                "target_pane_id": top_pane,
                "direction": "down",
                "ratio": 0.5,
                "focus": true,
            },
        })
        .to_string(),
    );
    let bottom_pane = split["result"]["pane"]["pane_id"]
        .as_str()
        .expect("bottom pane id");
    for (pane_id, label) in [(top_pane, "模块🭽界"), (bottom_pane, "notes")] {
        let response = send_json_request(
            &api_socket,
            &serde_json::json!({
                "id": format!("rename-{label}"),
                "method": "pane.rename",
                "params": {"pane_id": pane_id, "label": label},
            })
            .to_string(),
        );
        assert!(response.get("result").is_some(), "{response}");
    }
    send_pane_shell_command(&api_socket, top_pane, "printf 'TOP_NAMED_READY\\n'");
    send_pane_shell_command(&api_socket, bottom_pane, "printf 'BOTTOM_NAMED_READY\\n'");

    let (client, output) = attach_pixel_client(&config_home, &runtime_dir, &api_socket);
    assert!(wait_until(
        Duration::from_secs(8),
        Duration::from_millis(20),
        || {
            let bytes = output.lock().unwrap().bytes.clone();
            let screen = terminal_screen::text(completed_client_frame(&bytes), 80, 24);
            ["模块🭽界", "notes", "TOP_NAMED_READY", "BOTTOM_NAMED_READY"]
                .iter()
                .all(|value| screen.contains(value))
        }
    ));
    let bytes = completed_client_frame(&output.lock().unwrap().bytes).to_vec();
    let screen = terminal_screen::text(&bytes, 80, 24);
    let backgrounds = terminal_screen::explicit_backgrounds(&bytes, 80, 24);
    let foregrounds = terminal_screen::explicit_foregrounds(&bytes, 80, 24);
    let top_line = screen.lines().next().unwrap();
    let bottom_line = screen.lines().nth(12).unwrap();
    assert_eq!(top_line, "  模块🭽界");
    assert_eq!(bottom_line, "  notes");
    for (row, columns, background, foreground) in [
        (0, 1..10, [91, 89, 92], [255, 255, 255]),
        (12, 1..8, [169, 220, 118], [0, 0, 0]),
    ] {
        for x in columns {
            assert_eq!(backgrounds[row * 80 + x], Some(background));
            assert_eq!(foregrounds[row * 80 + x], Some(foreground));
        }
    }
    let green = if cfg!(target_os = "macos") {
        [179, 219, 130]
    } else {
        [169, 220, 118]
    };
    assert_pixel_border_images(
        &bytes,
        &[1360; 4],
        &[
            Some(PixelTitleSpec {
                range: 17..170,
                stroke: [91, 89, 92],
            }),
            None,
            Some(PixelTitleSpec {
                range: 17..136,
                stroke: green,
            }),
            None,
        ],
        &[],
    );
    if let Some(root) = std::env::var_os("HERDR_VISUAL_EVIDENCE_DIR") {
        let evidence = PathBuf::from(root).join("named-panes");
        fs::create_dir_all(&evidence).unwrap();
        fs::write(evidence.join("raw.ansi"), &bytes).unwrap();
        fs::write(evidence.join("screen.txt"), &screen).unwrap();
        fs::write(evidence.join("config.toml"), config).unwrap();
    }

    drop(client);
    drop(server);
    drop(cleanup);
}

#[test]
fn single_pane_keeps_rounded_focused_frame() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let cleanup = TestBaseCleanup(base.clone());
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");
    let config =
        PANE_BACKGROUND_CONFIG.replace("pane_borders = \"auto\"", "pane_borders = \"always\"");
    let server = spawn_server_with_config(
        &config_home,
        &runtime_dir,
        &api_socket,
        &client_socket,
        &config,
    );
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));
    let created = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "single-rounded-pane",
            "method": "workspace.create",
            "params": {"cwd": &base, "focus": true, "label": "single-pane-proof"},
        })
        .to_string(),
    );
    let pane = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("pane id");
    send_pane_shell_command(&api_socket, pane, "printf 'SINGLE_PANE_READY\\n'");
    let (client, output) = attach_pixel_client(&config_home, &runtime_dir, &api_socket);
    assert!(
        wait_until(Duration::from_secs(8), Duration::from_millis(20), || {
            let captured = output.lock().unwrap();
            let frame = completed_client_frame(&captured.bytes);
            terminal_screen::text(frame, 80, 24).contains("SINGLE_PANE_READY")
                && terminal_screen::explicit_backgrounds(frame, 80, 24)
                    == vec![Some([30, 30, 46]); 80 * 24]
        }),
        "single pane did not render"
    );
    let bytes = completed_client_frame(&output.lock().unwrap().bytes).to_vec();
    let screen = terminal_screen::text(&bytes, 80, 24);
    assert_pixel_border_images(&bytes, &[1360, 1360], &[], &[]);
    assert_eq!(
        terminal_screen::explicit_backgrounds(&bytes, 80, 24),
        vec![Some([30, 30, 46]); 80 * 24]
    );
    let foregrounds = terminal_screen::explicit_foregrounds(&bytes, 80, 24);
    for y in 1..23 {
        let symbols: Vec<char> = screen.lines().nth(y).unwrap().chars().collect();
        assert_eq!(symbols[0], '▏');
        assert_eq!(symbols[79], '▕');
        for x in [0, 79] {
            assert_eq!(
                foregrounds[y * 80 + x],
                Some([169, 220, 118]),
                "single pane stays focused"
            );
        }
    }
    if let Some(root) = std::env::var_os("HERDR_VISUAL_EVIDENCE_DIR") {
        let evidence = PathBuf::from(root).join("single-pane");
        fs::create_dir_all(&evidence).unwrap();
        fs::write(evidence.join("raw.ansi"), &bytes).unwrap();
        fs::write(evidence.join("screen.txt"), screen).unwrap();
        fs::write(evidence.join("config.toml"), config).unwrap();
    }
    drop(client);
    drop(server);
    drop(cleanup);
}

fn attach_pixel_client(
    config_home: &PathBuf,
    runtime_dir: &PathBuf,
    api_socket: &PathBuf,
) -> (SpawnedHerdr, SharedOutput) {
    let client = spawn_client_process_with_args_and_env(
        config_home,
        runtime_dir,
        api_socket,
        &["client"],
        &[("TERM_PROGRAM", "ghostty")],
    );
    let master = client._master.as_ref().expect("client PTY");
    master
        .resize(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 80 * 17,
            pixel_height: 24 * 36,
        })
        .expect("set exact host cell pixels");
    let output = spawn_pty_drain(master.try_clone_reader().expect("clone client PTY reader"));
    (client, output)
}

fn completed_client_frame(bytes: &[u8]) -> &[u8] {
    let end = b"\x1b[?2026l";
    bytes
        .windows(end.len())
        .rposition(|window| window == end)
        .map_or(&[], |index| &bytes[..index + end.len()])
}

struct PixelTitleSpec {
    range: std::ops::Range<usize>,
    stroke: [u8; 3],
}

fn assert_pixel_border_images(
    bytes: &[u8],
    expected_widths: &[usize],
    title_specs: &[Option<PixelTitleSpec>],
    inside_specs: &[Option<[u8; 3]>],
) {
    const CORNER: [&[u8; 12]; 12] = [
        b".......+++++",
        b".....++#####",
        b"....++##++++",
        b"...+##++    ",
        b"..+##+      ",
        b".++#+       ",
        b".+#+        ",
        b"+##+        ",
        b"+#+         ",
        b"+#+         ",
        b"+#+         ",
        b"+#+         ",
    ];
    use base64::Engine;
    let packets = regex::bytes::Regex::new(r"\x1b_G([^;]*f=100[^;]*);([^\x1b]*)\x1b\\").unwrap();
    let (inside, outside, green) = if cfg!(target_os = "macos") {
        ([30, 30, 45], [33, 31, 34], [179, 219, 130])
    } else {
        ([30, 30, 46], [34, 31, 34], [169, 220, 118])
    };
    let mut widths = Vec::new();
    let mut focused = 0;
    let mut top_edges = 0;
    for (image_index, packet) in packets.captures_iter(bytes).enumerate() {
        let title_spec = title_specs.get(image_index).and_then(Option::as_ref);
        let image_inside = inside_specs
            .get(image_index)
            .and_then(|color| *color)
            .unwrap_or(inside);
        let png = base64::engine::general_purpose::STANDARD
            .decode(&packet[2])
            .unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!(info.height, 36);
        assert_eq!(info.color_type, png::ColorType::Rgb);
        let width = info.width as usize;
        widths.push(width);
        let middle = |y: usize| &pixels[(y * width + width / 2) * 3..][..3];
        let top = middle(0) == outside;
        top_edges += usize::from(top);
        let stroke = middle(if top { 17 } else { 34 }).to_vec();
        assert!(
            stroke == [91, 89, 92] || stroke == green,
            "unexpected stroke {stroke:?}"
        );
        focused += usize::from(stroke == green);
        for y in 0..36_usize {
            for x in 0..width {
                let inset = x.min(width - 1 - x);
                let depth = if top {
                    y.saturating_sub(17)
                } else {
                    35_usize.saturating_sub(y)
                };
                let shape = if title_spec
                    .is_some_and(|title| title.range.contains(&x) && (4..32).contains(&y))
                {
                    b'#'
                } else if top && y < 17 {
                    b'.'
                } else if inset < 12 && depth < 12 {
                    CORNER[depth][inset]
                } else if depth < 2 || inset < 2 {
                    b'#'
                } else {
                    b' '
                };
                let actual = &pixels[(y * width + x) * 3..][..3];
                let expected: &[u8] = match shape {
                    b'.' => &outside,
                    b'#' => &stroke,
                    b' ' => &image_inside,
                    b'+' => {
                        assert!(
                            actual != outside && actual != image_inside && actual != stroke,
                            "antialiased pixel {x},{y}"
                        );
                        for channel in 0..3 {
                            let low = image_inside[channel]
                                .min(outside[channel])
                                .min(stroke[channel]);
                            let high = image_inside[channel]
                                .max(outside[channel])
                                .max(stroke[channel]);
                            assert!(
                                (low..=high).contains(&actual[channel]),
                                "corner blend {x},{y}"
                            );
                        }
                        continue;
                    }
                    _ => unreachable!(),
                };
                assert_eq!(actual, expected, "border pixel {x},{y}");
            }
        }
        if let Some(title) = title_spec {
            assert!(top, "title chip must be in a top strip");
            assert_eq!(stroke, title.stroke);
            for x in title.range.clone() {
                assert_eq!(&pixels[(4 * width + x) * 3..][..3], title.stroke);
                assert_eq!(&pixels[(31 * width + x) * 3..][..3], title.stroke);
            }
            assert_eq!(&pixels[(4 * width + title.range.end) * 3..][..3], outside);
            assert_eq!(
                &pixels[(17 * width + title.range.end) * 3..][..3],
                title.stroke
            );
            assert_eq!(&pixels[(31 * width + title.range.end) * 3..][..3], inside);
        }
    }
    if !title_specs.is_empty() {
        assert_eq!(title_specs.len(), widths.len());
    }
    widths.sort_unstable();
    assert_eq!(widths, expected_widths);
    assert_eq!(
        top_edges * 2,
        widths.len(),
        "one top and bottom edge per pane"
    );
    assert_eq!(focused, 2, "both focused horizontal edges stay green");
    eprintln!("pixel borders: {} PNGs, 12-pixel rounded corners, 2-pixel strokes, 17 + 0 = 17-pixel vertical gutter", widths.len());
}

fn read_output(output: &SharedOutput) -> String {
    output
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .text
        .clone()
}

/// Current captured byte length, used as a watermark so a test can search only
/// the output emitted *after* a trigger. The teardown markers also appear in
/// normal attach-phase output, so matching the whole buffer is meaningless.
fn output_len(output: &SharedOutput) -> usize {
    output.lock().unwrap_or_else(|p| p.into_inner()).text.len()
}

fn sidebar_right_edge(screen: &str) -> Option<usize> {
    screen.lines().find_map(|line| {
        line.chars()
            .position(|character| matches!(character, '│' | '▕'))
            .filter(|column| *column > 0)
    })
}

fn sidebar_row_click(screen: &str, label: &str) -> Vec<u8> {
    let sidebar_width = sidebar_right_edge(screen).expect("visible sidebar boundary");
    let row = screen
        .lines()
        .position(|line| {
            line.chars()
                .take(sidebar_width)
                .collect::<String>()
                .contains(label)
        })
        .unwrap_or_else(|| panic!("sidebar row {label:?} is not visible: {screen}"))
        + 1;
    format!("\x1b[<0;7;{row}M\x1b[<0;7;{row}m").into_bytes()
}

#[test]
fn sidebar_row_click_ignores_notice_borders() {
    let screen = "┌─────────────────────────┐\n│● Endpoint unavailable   │\n└─────────────────────────┘\n   · local-returned      │\n";
    assert_eq!(
        sidebar_row_click(screen, "local-returned"),
        b"\x1b[<0;7;4M\x1b[<0;7;4m"
    );
}

#[test]
fn sidebar_row_click_tracks_restored_workspace_count() {
    for restored in [false, true] {
        let screen = format!(
            " machines                │\n                         │\n ▾ Local                 │local-returned in pane output\n{}   · local-returned      └─────────────────\n",
            if restored { "   · restored            │\n" } else { "" }
        );
        let row = if restored { 5 } else { 4 };
        assert_eq!(
            sidebar_row_click(&screen, "local-returned"),
            format!("\x1b[<0;7;{row}M\x1b[<0;7;{row}m").into_bytes()
        );
    }
}

/// Spawns a server + real thin client under a PTY and waits until the client
/// has attached and rendered a frame. Returns the pieces plus a shared buffer
/// that keeps accumulating PTY output (including teardown) on a background
/// thread.
fn attach_thin_client(
    config_home: &PathBuf,
    runtime_dir: &PathBuf,
    api_socket: &PathBuf,
    client_socket: &PathBuf,
) -> (SpawnedHerdr, SpawnedHerdr, SharedOutput) {
    attach_thin_client_with_config(
        config_home,
        runtime_dir,
        api_socket,
        client_socket,
        "onboarding = false\n",
    )
}

fn attach_thin_client_with_config(
    config_home: &PathBuf,
    runtime_dir: &PathBuf,
    api_socket: &PathBuf,
    client_socket: &PathBuf,
    config: &str,
) -> (SpawnedHerdr, SpawnedHerdr, SharedOutput) {
    let spawned_server =
        spawn_server_with_config(config_home, runtime_dir, api_socket, client_socket, config);
    wait_for_socket(api_socket, Duration::from_secs(10));
    wait_for_socket(client_socket, Duration::from_secs(10));

    let thin_client = spawn_client_process(config_home, runtime_dir, api_socket);
    let reader = thin_client
        ._master
        .as_ref()
        .expect("thin client master")
        .try_clone_reader()
        .expect("clone client PTY reader");
    let output = spawn_pty_drain(reader);

    let deadline = Instant::now() + Duration::from_secs(8);
    let mut attached = false;
    while Instant::now() < deadline {
        let out = read_output(&output);
        if out.contains('\u{2500}')
            || out.contains("workspace")
            || out.contains("pane")
            || out.contains("terminal")
        {
            attached = true;
            break;
        }
        if out.to_lowercase().contains("herdr:") {
            break;
        }
        thread::sleep(Duration::from_millis(30));
    }
    assert!(
        attached,
        "thin client must attach and render a frame; output: {:?}",
        read_output(&output)
    );

    (spawned_server, thin_client, output)
}

#[test]
fn federated_launch_opens_local_directly_while_saved_ssh_is_unavailable() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = test_lock();
    for select_remote in [false, true] {
        let base = unique_test_dir();
        let config_home = base.join("config");
        let runtime_dir = base.join("runtime");
        let api_socket = runtime_dir.join("herdr.sock");
        fs::create_dir_all(config_home.join(app_dir_name())).unwrap();
        fs::write(
            config_home.join(app_dir_name()).join("config.toml"),
            "onboarding = false\n",
        )
        .unwrap();
        let catalog_dir = runtime_dir
            .join("state")
            .join(app_dir_name())
            .join("client");
        fs::create_dir_all(&catalog_dir).unwrap();
        let profile = "0123456789abcdef0123456789abcdef";
        fs::write(catalog_dir.join("endpoints.json"), serde_json::json!({
            "version": 1, "selected_profile": select_remote.then_some(profile),
            "ssh": [{"id": profile, "label": "Unavailable remote", "target": "test-only", "session": "default", "enabled": true}],
        }).to_string()).unwrap();
        let bin = base.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("ssh"), "#!/bin/sh\nexit 255\n").unwrap();
        fs::set_permissions(bin.join("ssh"), fs::Permissions::from_mode(0o700)).unwrap();
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );

        // Exercise both auto-start and a subsequent attach to the healthy Local server.
        for args in [&[][..], &["client"][..]] {
            let client = spawn_client_process_with_args_and_env(
                &config_home,
                &runtime_dir,
                &api_socket,
                args,
                &[("PATH", &path)],
            );
            let output =
                spawn_pty_drain(client._master.as_ref().unwrap().try_clone_reader().unwrap());
            wait_for_socket(&api_socket, Duration::from_secs(10));
            assert!(wait_until(
                Duration::from_secs(10),
                Duration::from_millis(20),
                || { read_output(&output).contains("Local") }
            ));
            let mut input = client._master.as_ref().unwrap().take_writer().unwrap();
            // Input is gated until Local's active surface is ready, and that readiness can lag
            // the first rendered frame (the unavailable remote must not extend the wait). Retry
            // the write instead of assuming a single write lands, matching the recovered-Local
            // path below.
            assert!(wait_until(Duration::from_secs(10), Duration::from_millis(20), || {
                if read_output(&output).contains("LOCAL_DIRECT_READY") {
                    return true;
                }
                input
                    .write_all(b"printf 'LOCAL_%s\\n' DIRECT_READY\r")
                    .unwrap();
                false
            }), "Local must accept input without waiting for SSH (remote selected: {select_remote}): {}", read_output(&output));
            let text = read_output(&output);
            assert!(!text.contains("Local: connecting"), "{text}");
            assert!(!text.contains("Local: reconnecting"), "{text}");
            drop(input);
            drop(client);
        }
        let _ = send_json_request(
            &api_socket,
            r#"{"id":"stop","method":"server.stop","params":{}}"#,
        );
        cleanup_test_base(&base);
    }
}

#[test]
fn federated_client_starts_without_local_and_survives_its_restart() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");
    let remote_config = base.join("remote-config");
    let remote_runtime = base.join("remote-runtime");
    let remote_api = remote_runtime.join("herdr.sock");
    let remote_client = remote_runtime.join("herdr-client.sock");
    let mut remote_server =
        spawn_server(&remote_config, &remote_runtime, &remote_api, &remote_client);
    wait_for_socket(&remote_api, Duration::from_secs(10));
    wait_for_socket(&remote_client, Duration::from_secs(10));
    let created = send_json_request(
        &remote_api,
        &serde_json::json!({
            "id": "remote-workspace", "method": "workspace.create",
            "params": {"cwd": base, "focus": true, "label": "remote-ready"},
        })
        .to_string(),
    );
    let remote_pane = created["result"]["root_pane"]["pane_id"].as_str().unwrap();
    send_pane_shell_command(&remote_api, remote_pane, "printf 'REMOTE_INITIAL_FRAME\\n'");

    fs::create_dir_all(config_home.join(app_dir_name())).unwrap();
    fs::write(
        config_home.join(app_dir_name()).join("config.toml"),
        "onboarding = false\n",
    )
    .unwrap();
    let catalog_dir = runtime_dir
        .join("state")
        .join(app_dir_name())
        .join("client");
    fs::create_dir_all(&catalog_dir).unwrap();
    let profile = "0123456789abcdef0123456789abcdef";
    fs::write(catalog_dir.join("endpoints.json"), serde_json::json!({
        "version": 1, "selected_profile": profile,
        "ssh": [{"id": profile, "label": "Test remote", "target": "test-only", "session": "default", "enabled": true}],
    }).to_string()).unwrap();

    // The SSH executable is private to this client. Discovery and the stdio bridge run the real
    // binary against a second disposable local server, never the developer's saved hosts.
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(base.join("home")).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_herdr"), bin.join("herdr")).unwrap();
    let quote =
        |path: &std::path::Path| format!("'{}'", path.display().to_string().replace('\'', "'\\''"));
    let ssh_commands = base.join("ssh-commands");
    let bridge_pid = base.join("bridge-pid");
    fs::write(bin.join("ssh"), format!(
        "#!/bin/sh\nexport HOME={} XDG_CONFIG_HOME={} XDG_RUNTIME_DIR={} HERDR_SOCKET_PATH={}\nunset HERDR_CLIENT_SOCKET_PATH HERDR_SESSION\nfor arg do last=\"$arg\"; done\nprintf '%s\\n' \"$last\" >> {}\ncase \"$last\" in *remote-client-bridge*) printf '%s\\n' \"$$\" > {};; esac\nexec /bin/sh -c \"$last\"\n",
        quote(&base.join("home")), quote(&remote_config), quote(&remote_runtime), quote(&remote_api), quote(&ssh_commands), quote(&bridge_pid),
    )).unwrap();
    fs::set_permissions(bin.join("ssh"), fs::Permissions::from_mode(0o700)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut client = spawn_client_process_with_args_and_env(
        &config_home,
        &runtime_dir,
        &api_socket,
        &["client"],
        &[("PATH", &path)],
    );
    let output = spawn_pty_drain(client._master.as_ref().unwrap().try_clone_reader().unwrap());
    let screen_text = || {
        let bytes = output
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .bytes
            .clone();
        terminal_screen::text(&bytes, 80, 24)
    };
    assert!(
        wait_until(Duration::from_secs(12), Duration::from_millis(20), || {
            screen_text().contains("REMOTE_INITIAL_FRAME")
        }),
        "remote must be usable before Local exists: {}",
        read_output(&output)
    );
    assert!(
        fs::read_to_string(&ssh_commands)
            .unwrap()
            .contains("remote-client-bridge --idle-timeout-v1"),
        "saved machine discovery must opt into the advertised bridge idle timeout"
    );

    let mut input = client._master.as_ref().unwrap().take_writer().unwrap();
    send_pane_shell_command(&remote_api, remote_pane, "reconnect_survivor=ALIVE");
    for cycle in 1..=3 {
        let pid: libc::pid_t = fs::read_to_string(&bridge_pid)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
        let marker = format!("REMOTE_RECONNECTED_{cycle}");
        send_pane_shell_command(&remote_api, remote_pane, &format!("printf '{marker}\\n'"));
        assert!(
            wait_until(Duration::from_secs(15), Duration::from_millis(20), || {
                screen_text().contains(&marker)
            }),
            "remote reconnect {cycle} must restore the visible screen without switching machines"
        );
        assert!(
            wait_until(Duration::from_secs(8), Duration::from_millis(100), || {
                if screen_text().contains(&format!("REMOTE_ALIVE_INPUT_{cycle}")) {
                    return true;
                }
                write!(
                    input,
                    "printf 'REMOTE_%s_INPUT_{cycle}\\n' \"$reconnect_survivor\"\r"
                )
                .unwrap();
                false
            }),
            "remote reconnect {cycle} must restore visible input and preserve the shell"
        );
    }

    let mut local = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    let created = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "local-workspace", "method": "workspace.create",
            "params": {"cwd": base, "focus": true, "label": "local-online"},
        })
        .to_string(),
    );
    assert_eq!(created["result"]["type"], "workspace_created");
    assert!(wait_until(
        Duration::from_secs(10),
        Duration::from_millis(20),
        || screen_text().contains("local-online")
    ));

    local.child.kill().unwrap();
    local.close_master();
    drop(local);
    assert!(
        wait_until(Duration::from_secs(8), Duration::from_millis(20), || {
            if screen_text().contains("REMOTE_SURVIVED") {
                return true;
            }
            input
                .write_all(b"printf 'REMOTE_%s\\n' SURVIVED\r")
                .unwrap();
            false
        }),
        "Local loss must not interrupt remote input or output: {}",
        screen_text()
    );
    assert!(client.child.try_wait().unwrap().is_none());

    let restarted = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    let created = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "local-returned", "method": "workspace.create",
            "params": {"cwd": base, "focus": true, "label": "local-returned"},
        })
        .to_string(),
    );
    assert_eq!(created["result"]["type"], "workspace_created");
    assert!(
        wait_until(Duration::from_secs(12), Duration::from_millis(20), || {
            screen_text().contains("local-returned")
        }),
        "Local must reconnect with fresh metadata"
    );
    input
        .write_all(b"printf 'REMOTE_%s\\n' STILL_SELECTED\r")
        .unwrap();
    assert!(
        wait_until(Duration::from_secs(8), Duration::from_millis(20), || {
            screen_text().contains("REMOTE_STILL_SELECTED")
        }),
        "Local recovery must not steal selection: {}",
        screen_text()
    );
    let local_pane = created["result"]["root_pane"]["pane_id"].as_str().unwrap();
    send_pane_shell_command(
        &api_socket,
        local_pane,
        "printf 'LOCAL_WHILE_REMOTE_STALLED\\n'",
    );
    {
        struct ResumeBridge(libc::pid_t);
        impl Drop for ResumeBridge {
            fn drop(&mut self) {
                unsafe { libc::kill(self.0, libc::SIGCONT) };
            }
        }
        let bridge: libc::pid_t = fs::read_to_string(&bridge_pid)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(bridge, libc::SIGSTOP) }, 0);
        let _resume_bridge = ResumeBridge(bridge);
        input
            .write_all(&sidebar_row_click(&screen_text(), "local-returned"))
            .unwrap();
        assert!(
            wait_until(Duration::from_secs(3), Duration::from_millis(20), || {
                screen_text().contains("LOCAL_WHILE_REMOTE_STALLED")
            }),
            "one Local selection must not wait for the remote bridge: {}",
            screen_text()
        );
        assert!(
            wait_until(Duration::from_secs(3), Duration::from_millis(100), || {
                if screen_text().contains("LOCAL_INPUT_WHILE_REMOTE_STALLED") {
                    return true;
                }
                input
                    .write_all(b"printf 'LOCAL_%s\\n' INPUT_WHILE_REMOTE_STALLED\r")
                    .unwrap();
                false
            }),
            "Local input must become usable while the remote bridge remains stopped"
        );
    }
    input
        .write_all(&sidebar_row_click(&screen_text(), "remote-ready"))
        .unwrap();
    assert!(wait_until(
        Duration::from_secs(10),
        Duration::from_millis(20),
        || screen_text().contains("REMOTE_STILL_SELECTED")
    ));

    let watermark = output_len(&output);
    remote_server.child.kill().unwrap();
    assert!(
        wait_until(Duration::from_secs(10), Duration::from_millis(20), || {
            read_output(&output)[watermark..].contains("reconnecting")
        }),
        "the selected remote must be marked disconnected"
    );
    let text = read_output(&output);
    assert!(
        text.rfind("\x1b[?1000h") > text.rfind("\x1b[?1000l"),
        "losing the selected remote must keep host mouse reporting enabled"
    );

    send_pane_shell_command(
        &api_socket,
        local_pane,
        "printf 'LOCAL_RECOVERED_SURFACE\\n'",
    );
    // Select the fresh workspace below Local's restored workspace.
    // A fast shutdown may leave no saved workspace, so locate the actual row.
    input
        .write_all(&sidebar_row_click(&screen_text(), "local-returned"))
        .unwrap();
    assert!(
        wait_until(Duration::from_secs(10), Duration::from_millis(20), || {
            screen_text().contains("LOCAL_RECOVERED_SURFACE")
        }),
        "recovered Local must be selectable: {}",
        screen_text()
    );
    // A coherent frame precedes the final host-effects fence; input stays gated until then.
    assert!(
        wait_until(Duration::from_secs(8), Duration::from_millis(100), || {
            if screen_text().contains("LOCAL_INPUT_RECOVERED") {
                return true;
            }
            input
                .write_all(b"printf 'LOCAL_%s\\n' INPUT_RECOVERED\r")
                .unwrap();
            false
        }),
        "recovered Local must accept input: {}",
        screen_text()
    );
    drop(input);
    drop(client);
    drop(restarted);
    drop(remote_server);
    cleanup_test_base(&base);
}

#[test]
fn client_shell_detaches_restores_and_freshly_reattaches_to_current_state() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let mut server = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let created = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "client-shell-lifecycle-workspace",
            "method": "workspace.create",
            "params": {"cwd": base, "focus": true, "label": "shell-lifecycle"},
        })
        .to_string(),
    );
    assert_eq!(created["result"]["type"], "workspace_created", "{created}");
    let pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("root pane id")
        .to_string();
    send_pane_shell_command(&api_socket, &pane_id, "printf 'SHELL_LIFECYCLE_INITIAL\\n'");

    let mut client_a = spawn_client_shell_process(&config_home, &runtime_dir, &api_socket);
    let output_a = spawn_pty_drain(
        client_a
            ._master
            .as_ref()
            .expect("first client shell PTY")
            .try_clone_reader()
            .expect("clone first client shell reader"),
    );
    assert!(
        wait_until(Duration::from_secs(8), Duration::from_millis(20), || {
            let output = read_output(&output_a);
            output.contains("shell-lifecycle") && output.contains("SHELL_LIFECYCLE_INITIAL")
        }),
        "client shell should compose one coherent snapshot and pane surface; output: {:?}",
        read_output(&output_a)
    );

    let detach_watermark = output_len(&output_a);
    client_a
        ._master
        .as_ref()
        .expect("first client shell PTY")
        .take_writer()
        .expect("first client shell writer")
        .write_all(b"\x02q")
        .expect("detach first client shell");
    let detach_output = drain_until_client_exits(&mut client_a, &output_a, detach_watermark);
    assert!(
        output_has_mouse_teardown(&detach_output),
        "client shell should restore the host terminal after detach; output: {detach_output:?}"
    );
    assert!(
        ping_socket(&api_socket).contains("pong"),
        "server should remain alive after client shell detach"
    );
    drop(client_a);

    send_pane_shell_command(
        &api_socket,
        &pane_id,
        "printf 'SHELL_LIFECYCLE_DETACHED\\n'",
    );
    let mut client_b = spawn_client_shell_process(&config_home, &runtime_dir, &api_socket);
    let output_b = spawn_pty_drain(
        client_b
            ._master
            .as_ref()
            .expect("reattached client shell PTY")
            .try_clone_reader()
            .expect("clone reattached client shell reader"),
    );
    assert!(
        wait_until(Duration::from_secs(8), Duration::from_millis(20), || {
            let output = read_output(&output_b);
            output.contains("shell-lifecycle") && output.contains("SHELL_LIFECYCLE_DETACHED")
        }),
        "fresh client shell should receive current state and detached-period output; output: {:?}",
        read_output(&output_b)
    );

    let disconnect_watermark = output_len(&output_b);
    if let Some(pid) = server.child.process_id() {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    server.close_master();
    let disconnect_output =
        drain_until_client_exits(&mut client_b, &output_b, disconnect_watermark);
    assert!(
        output_has_mouse_teardown(&disconnect_output),
        "client shell should restore the host terminal after endpoint loss; output: {disconnect_output:?}"
    );
    assert!(
        disconnect_output
            .to_lowercase()
            .contains("lost connection to server"),
        "client shell should explain endpoint loss; output: {disconnect_output:?}"
    );

    drop(server);
    cleanup_spawned_herdr(client_b, base);
}

fn captured_window_titles(output: &SharedOutput) -> Vec<String> {
    read_output(output)
        .split("\x1b]0;")
        .skip(1)
        .filter_map(|suffix| {
            suffix
                .split_once('\x07')
                .map(|(title, _)| title.to_string())
        })
        .collect()
}

fn wait_for_window_title(output: &SharedOutput, expected_suffix: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(title) = captured_window_titles(output)
            .into_iter()
            .find(|title| title.ends_with(expected_suffix))
        {
            return title;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "outer window title ending in {expected_suffix:?} was not emitted; titles: {:?}; output: {:?}",
        captured_window_titles(output),
        read_output(output)
    );
}

fn wait_for_pane_terminal_title(socket_path: &PathBuf, pane_id: &str, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let request = serde_json::json!({
            "id": "window-title-pane-get",
            "method": "pane.get",
            "params": {"pane_id": pane_id},
        });
        let response = send_json_request(socket_path, &request.to_string());
        if response["result"]["pane"]["terminal_title"].as_str() == Some(expected) {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("pane {pane_id} did not report terminal title {expected:?}");
}

fn send_pane_shell_command(socket_path: &PathBuf, pane_id: &str, command: &str) {
    let request = serde_json::json!({
        "id": "window-title-command",
        "method": "pane.send_input",
        "params": {
            "pane_id": pane_id,
            "text": command,
            "keys": ["Enter"],
        }
    });
    let response = send_json_request(socket_path, &request.to_string());
    assert_eq!(response["result"]["type"], "ok", "{response}");
}

#[test]
fn configured_window_title_tracks_all_tokens_and_focused_osc_only() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");
    let (server, client, output) = attach_thin_client_with_config(
        &config_home,
        &runtime_dir,
        &api_socket,
        &client_socket,
        "onboarding = false\n[ui]\nwindow_title = \"H={hostname}|W={workspace}|T={tab}|P={pane}|O={terminal_title}\"\n",
    );

    let created = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "create-workspace",
            "method": "workspace.create",
            "params": {"cwd": base, "focus": true},
        })
        .to_string(),
    );
    assert_eq!(created["result"]["type"], "workspace_created", "{created}");
    let workspace_id = created["result"]["workspace"]["workspace_id"]
        .as_str()
        .expect("workspace id")
        .to_string();
    let pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("pane id")
        .to_string();
    let tab_id = created["result"]["tab"]["tab_id"]
        .as_str()
        .expect("tab id")
        .to_string();

    for request in [
        serde_json::json!({
            "id": "rename-workspace",
            "method": "workspace.rename",
            "params": {"workspace_id": workspace_id, "label": "space-a"},
        }),
        serde_json::json!({
            "id": "rename-tab",
            "method": "tab.rename",
            "params": {"tab_id": tab_id, "label": "tab-a"},
        }),
        serde_json::json!({
            "id": "rename-pane",
            "method": "pane.rename",
            "params": {"pane_id": pane_id, "label": "pane-a"},
        }),
    ] {
        let response = send_json_request(&api_socket, &request.to_string());
        assert!(response.get("result").is_some(), "{response}");
    }

    let renamed = wait_for_window_title(&output, "|W=space-a|T=tab-a|P=pane-a|O=");
    assert!(renamed.starts_with("H="));
    assert!(
        !renamed.starts_with("H=|"),
        "hostname token was empty: {renamed}"
    );

    send_pane_shell_command(&api_socket, &pane_id, r"printf '\033]0;building\007'");
    wait_for_window_title(&output, "|W=space-a|T=tab-a|P=pane-a|O=building");

    let second_tab = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "second-tab",
            "method": "tab.create",
            "params": {"workspace_id": workspace_id, "focus": true},
        })
        .to_string(),
    );
    assert_eq!(second_tab["result"]["type"], "tab_created", "{second_tab}");
    let second_pane_id = second_tab["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("second pane id")
        .to_string();
    wait_for_window_title(&output, "|W=space-a|T=2|P=|O=");
    let titles_before_hidden_update = captured_window_titles(&output).len();
    send_pane_shell_command(&api_socket, &pane_id, r"printf '\033]0;hidden update\007'");
    // Intentionally consume the AppState title through a read-only request
    // before the queued source is handled.
    wait_for_pane_terminal_title(&api_socket, &pane_id, "hidden update");
    send_pane_shell_command(
        &api_socket,
        &second_pane_id,
        r"printf '\033]0;foreground marker\007'",
    );
    wait_for_window_title(&output, "|W=space-a|T=2|P=|O=foreground marker");
    assert!(
        captured_window_titles(&output)[titles_before_hidden_update..]
            .iter()
            .all(|title| !title.ends_with("|O=hidden update")),
        "a hidden pane title reached the outer terminal"
    );

    let focused = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "focus-first-tab",
            "method": "tab.focus",
            "params": {"tab_id": tab_id},
        })
        .to_string(),
    );
    assert_eq!(focused["result"]["tab"]["focused"], true, "{focused}");
    wait_for_window_title(&output, "|W=space-a|T=tab-a|P=pane-a|O=hidden update");

    drop(server);
    cleanup_spawned_herdr(client, base);
}

/// Polls until the client exits, then returns only the output captured after
/// the `since` byte watermark. Panics if the client does not exit within the
/// deadline.
fn drain_until_client_exits(
    thin_client: &mut SpawnedHerdr,
    output: &SharedOutput,
    since: usize,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut exited = false;
    while Instant::now() < deadline {
        if thin_client.child.try_wait().ok().flatten().is_some() {
            exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    // Give the reader thread a beat to flush trailing teardown bytes.
    thread::sleep(Duration::from_millis(100));
    let full = read_output(output);
    assert!(exited, "thin client should exit; output: {full:?}");
    full.get(since..).unwrap_or_default().to_string()
}

/// Attaches a thin client, runs `trigger` to force an exit, and asserts the
/// client emits the mouse teardown after that point. The teardown markers also
/// appear in normal attach output, so only bytes emitted after the trigger
/// (past the watermark) count.
fn assert_client_restores_terminal(trigger: impl FnOnce(&mut SpawnedHerdr, &mut SpawnedHerdr)) {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let (mut spawned_server, mut thin_client, pty_output) =
        attach_thin_client(&config_home, &runtime_dir, &api_socket, &client_socket);

    let since = output_len(&pty_output);
    trigger(&mut spawned_server, &mut thin_client);

    let output = drain_until_client_exits(&mut thin_client, &pty_output, since);
    assert!(
        output_has_mouse_teardown(&output),
        "client must emit mouse teardown after trigger; output after trigger: {output:?}"
    );

    // SpawnedHerdr::Drop kills and reaps both processes with a bounded wait.
    drop(spawned_server);
    cleanup_spawned_herdr(thin_client, base);
}

/// The `--remote` ssh-death path: killing the bridge closes the socket, the
/// client sees EOF and unwinds normally, so the terminal is restored. This is
/// the path that does NOT deliver a signal to the client. Guards against a
/// regression that would leave mouse reporting on after an ssh disconnect.
#[test]
fn client_restores_terminal_on_server_eof() {
    assert_client_restores_terminal(|server, _client| {
        // Kill the server unexpectedly; the client socket closes and the
        // client reader hits EOF, mirroring the ssh bridge dying under
        // `herdr --remote`.
        if let Some(pid) = server.child.process_id() {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
        server.close_master();
    });
}

/// A direct SIGHUP/SIGTERM with a writable terminal follows the graceful quit
/// path and emits the terminal teardown. Actual terminal-window closure also
/// makes the PTY unwritable and is covered separately below.
#[test]
fn client_restores_terminal_on_sighup() {
    assert_client_restores_terminal(|_server, client| {
        let pid = client.child.process_id().expect("thin client pid") as libc::pid_t;
        unsafe {
            libc::kill(pid, libc::SIGHUP);
        }
    });
}

fn read_until_client_attaches(client: &SpawnedHerdr) -> String {
    let master = client._master.as_ref().expect("thin client master");
    let fd = master.as_raw_fd().expect("thin client PTY file descriptor");
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    assert_ne!(flags, -1, "read thin client PTY flags");
    assert_ne!(
        unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
        -1,
        "make thin client PTY nonblocking"
    );

    let mut reader = master.try_clone_reader().expect("clone client PTY reader");
    let mut output = String::new();
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        let mut buf = [0u8; 4096];
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => output.push_str(&String::from_utf8_lossy(&buf[..n])),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(err) => panic!("read thin client PTY: {err}"),
        }
        if output.contains('\u{2500}')
            || output.contains("workspace")
            || output.contains("pane")
            || output.contains("terminal")
        {
            return output;
        }
    }
    panic!("thin client must attach and render a frame; output: {output:?}");
}

#[test]
fn client_exits_cleanly_when_terminal_and_transport_hang_up() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let mut spawned_server = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let mut thin_client = spawn_client_process(&config_home, &runtime_dir, &api_socket);
    read_until_client_attaches(&thin_client);

    // Freeze the client so the dead terminal and transport EOF are both
    // observable when it resumes, making the `--remote` shutdown race deterministic.
    let client_pid = thin_client.child.process_id().expect("thin client pid") as libc::pid_t;
    assert_eq!(
        unsafe { libc::kill(client_pid, libc::SIGSTOP) },
        0,
        "stop thin client"
    );
    let server_pid = spawned_server.child.process_id().expect("server pid") as libc::pid_t;
    assert_eq!(
        unsafe { libc::kill(server_pid, libc::SIGKILL) },
        0,
        "kill server transport"
    );
    spawned_server.close_master();
    thin_client.close_master();
    assert_eq!(
        unsafe { libc::kill(client_pid, libc::SIGCONT) },
        0,
        "resume thin client"
    );

    let deadline = Instant::now() + Duration::from_secs(12);
    let status = loop {
        if let Some(status) = thin_client.child.try_wait().expect("poll thin client") {
            break Some(status);
        }
        if Instant::now() >= deadline {
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };

    drop(spawned_server);
    cleanup_spawned_herdr(thin_client, base);

    let status = status.expect("thin client should exit after terminal and transport hang up");
    assert!(
        status.success(),
        "thin client should exit cleanly after terminal and transport hang up, got {status}"
    );
}

#[test]
fn client_exits_cleanly_when_terminal_hangs_up() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let spawned_server = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let mut thin_client = spawn_client_process(&config_home, &runtime_dir, &api_socket);
    let attached_output = read_until_client_attaches(&thin_client);

    // Closing the final PTY master models the outer terminal disappearing: the
    // foreground client receives SIGHUP and writes to stdout/stderr fail.
    thin_client.close_master();
    let deadline = Instant::now() + Duration::from_secs(12);
    let status = loop {
        if let Some(status) = thin_client.child.try_wait().expect("poll thin client") {
            break Some(status);
        }
        if Instant::now() >= deadline {
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let server_response = ping_socket(&api_socket);

    drop(spawned_server);
    cleanup_spawned_herdr(thin_client, base);

    let status = status.unwrap_or_else(|| {
        panic!("thin client did not exit after PTY hangup; attach output: {attached_output:?}")
    });
    assert!(
        status.success(),
        "thin client should exit cleanly after PTY hangup, got {status}; attach output: {attached_output:?}"
    );
    assert!(
        server_response.contains("pong"),
        "server should survive client PTY hangup: {server_response}"
    );
}

#[test]
fn client_receives_pane_surface_after_pane_output() {
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let spawned = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let mut stream = UnixStream::connect(&client_socket).expect("should connect to client socket");
    let (version, error) = client_shell_handshake(&mut stream, CURRENT_PROTOCOL, 54, 23)
        .expect("handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "{error:?}");
    wait_for_client_shell_bootstrap(&mut stream, Duration::from_secs(10))
        .expect("initial client shell bootstrap");

    let created = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "create-output-workspace",
            "method": "workspace.create",
            "params": {"label": "output", "focus": true}
        })
        .to_string(),
    );
    let pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .expect("root pane id");
    assert!(wait_for_message_variant(
        &mut stream,
        Duration::from_secs(5),
        SERVER_MESSAGE_PANE_SURFACE,
    )
    .expect("wait for created workspace surface"));

    let sent = send_json_request(
        &api_socket,
        &serde_json::json!({
            "id": "send-output",
            "method": "pane.send_text",
            "params": {"pane_id": pane_id, "text": "printf 'test-output\\n'\\n"}
        })
        .to_string(),
    );
    assert!(sent.get("error").is_none(), "{sent}");
    assert!(
        wait_for_message_variants(
            &mut stream,
            Duration::from_secs(5),
            &[
                SERVER_MESSAGE_PANE_SURFACE,
                SERVER_MESSAGE_PANE_SURFACE_PATCH,
            ],
        )
        .expect("wait for post-output pane surface"),
        "should receive a pane surface update after pane output"
    );

    cleanup_spawned_herdr(spawned, base);
}

#[test]
fn pane_spawn_cwd_fallback_in_server() {
    // Pane spawn failure cwd fallback in server context.
    // This test verifies that the server can start even with invalid
    // session data pointing to non-existent directories.
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");
    let data_dir = config_home.join(app_dir_name());
    let missing_cwd = base.join("missing-cwd-for-test");
    let missing_cwd = missing_cwd.to_str().expect("test cwd should be UTF-8");
    fs::create_dir_all(&data_dir).unwrap();
    let session = serde_json::json!({
        "version": 2,
        "workspaces": [{
            "custom_name": "missing-cwd",
            "layout": { "Pane": 0 },
            "panes": { "0": { "cwd": missing_cwd } },
            "zoomed": false,
            "focused": 0,
            "root_pane": 0
        }],
        "active": 0,
        "selected": 0
    });
    fs::write(
        data_dir.join("session.json"),
        serde_json::to_vec_pretty(&session).unwrap(),
    )
    .unwrap();

    let spawned = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let workspaces = send_json_request(
        &api_socket,
        r#"{"id":"workspace_list","method":"workspace.list","params":{}}"#,
    );
    let restored_workspace = workspaces["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|workspace| workspace["label"] == "missing-cwd")
        .expect("server should restore workspace with missing pane cwd");
    let workspace_id = restored_workspace["workspace_id"]
        .as_str()
        .expect("restored workspace should have public id");
    let pane_id = first_pane_id_in_workspace(&api_socket, workspace_id);
    let pane = send_json_request(
        &api_socket,
        &format!(r#"{{"id":"pane_get","method":"pane.get","params":{{"pane_id":"{pane_id}"}}}}"#),
    );
    assert_eq!(pane["result"]["pane"]["workspace_id"], workspace_id);
    let cwd = pane["result"]["pane"]["cwd"]
        .as_str()
        .expect("restored pane should report fallback cwd");
    assert_ne!(cwd, missing_cwd);
    assert!(
        std::path::Path::new(cwd).exists(),
        "fallback cwd should exist: {cwd}"
    );

    let client_shell = spawn_client_shell_process(&config_home, &runtime_dir, &api_socket);
    let output = spawn_pty_drain(
        client_shell
            ._master
            .as_ref()
            .expect("restored client shell PTY")
            .try_clone_reader()
            .expect("clone restored client shell reader"),
    );
    assert!(
        wait_until(Duration::from_secs(8), Duration::from_millis(20), || {
            read_output(&output).contains("missing-cwd")
        }),
        "client shell should render the restored session; output: {:?}",
        read_output(&output)
    );

    drop(spawned);
    cleanup_spawned_herdr(client_shell, base);
}

#[test]
fn graceful_shutdown_sends_server_shutdown_to_client() {
    // Issue 2 fix: SIGINT triggers initiate_shutdown → ServerShutdown
    // broadcast to all clients before the server exits.
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    let mut spawned = spawn_server(&config_home, &runtime_dir, &api_socket, &client_socket);
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let mut stream = UnixStream::connect(&client_socket).expect("should connect to client socket");
    let (version, error) = client_shell_handshake(&mut stream, CURRENT_PROTOCOL, 54, 23)
        .expect("handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "{error:?}");
    wait_for_client_shell_bootstrap(&mut stream, Duration::from_secs(5))
        .expect("client shell bootstrap");

    // Send SIGINT to the server process to trigger graceful shutdown.
    if let Some(pid) = spawned.child.process_id() {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGINT);
        }
    }

    // The client should receive a ServerShutdown message
    // before the connection is closed, not just an abrupt EOF.
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let result = read_server_message(&mut stream);
    match result {
        Ok((variant, _payload)) => {
            assert_eq!(
                variant, SERVER_MESSAGE_SERVER_SHUTDOWN,
                "expected ServerShutdown, got variant {variant}"
            );
        }
        Err(e) => {
            panic!("expected ServerShutdown message before connection close, got error: {e}");
        }
    }

    // Wait for the server to exit.
    spawned.close_master();
    let _ = spawned.child.wait();

    drop(spawned);
    cleanup_test_base(&base);
}

#[test]
fn client_receives_notify_on_agent_state_change() {
    // Notification events (sound/toast) are forwarded as
    // ServerMessage::Notify to connected clients when an agent state change
    // is triggered via the API (pane.report_agent).
    let _lock = test_lock();
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let api_socket = runtime_dir.join("herdr.sock");
    let client_socket = runtime_dir.join("herdr-client.sock");

    // Enable toast and sound in config so the server produces notifications.
    fs::create_dir_all(config_home.join(app_dir_name())).unwrap();
    fs::write(
        config_home.join(app_dir_name()).join("config.toml"),
        "onboarding = false\n[ui.toast]\nenabled = true\n[ui.sound]\nenabled = true\n",
    )
    .unwrap();
    fs::create_dir_all(&runtime_dir).unwrap();
    register_runtime_dir(&runtime_dir);

    // Spawn the server directly (not using spawn_server helper because it
    // overwrites the config file with a minimal one).
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_herdr"));
    cmd.arg("server");
    cmd.env("XDG_CONFIG_HOME", &config_home);
    cmd.env("XDG_RUNTIME_DIR", &runtime_dir);
    cmd.env("HERDR_SOCKET_PATH", &api_socket);
    cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
    cmd.env("SHELL", "/bin/sh");
    cmd.env_remove("HERDR_ENV");

    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_herdr_pid(child.process_id());
    drop(pair.slave);

    let spawned = SpawnedHerdr {
        _master: Some(pair.master),
        child,
    };
    wait_for_socket(&api_socket, Duration::from_secs(10));
    wait_for_socket(&client_socket, Duration::from_secs(10));

    let mut stream = UnixStream::connect(&client_socket).expect("should connect");
    let (version, error) = client_shell_handshake(&mut stream, CURRENT_PROTOCOL, 54, 23)
        .expect("handshake should succeed");
    assert_eq!(version, CURRENT_PROTOCOL);
    assert!(error.is_none(), "{error:?}");
    wait_for_client_shell_bootstrap(&mut stream, Duration::from_secs(5))
        .expect("client shell bootstrap");

    // Create a workspace via the API.
    let mut ws_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let request = r#"{"id":"1","method":"workspace.create","params":{}}"#;
    writeln!(ws_stream, "{}", request).unwrap();
    let mut reader = BufReader::new(ws_stream);
    let mut ws_response = String::new();
    reader.read_line(&mut ws_response).unwrap();

    // Extract the workspace ID and pane ID from the response.
    let ws_id = ws_response
        .split('"')
        .find(|s| s.starts_with("w_"))
        .unwrap_or("w_1")
        .to_string();

    // Get pane list to find a pane ID.
    let mut pane_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let pane_request =
        format!(r#"{{"id":"2","method":"pane.list","params":{{"workspace_id":"{ws_id}"}}}}"#);
    writeln!(pane_stream, "{}", pane_request).unwrap();
    let mut pane_reader = BufReader::new(pane_stream);
    let mut pane_response = String::new();
    pane_reader.read_line(&mut pane_response).unwrap();

    // Extract first pane ID (format: p_<ws>_<pane>).
    let pane_id = pane_response
        .split('"')
        .find(|s| s.starts_with("p_"))
        .unwrap_or("p_1_1")
        .to_string();

    // Report agent as Blocked via the API — this should trigger a
    // ServerMessage::Notify with kind=Sound (Request sound).
    let mut report_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let report_request = format!(
        r#"{{"id":"3","method":"pane.report_agent","params":{{"pane_id":"{pane_id}","agent":"pi","state":"blocked","source":"test"}}}}"#
    );
    writeln!(report_stream, "{}", report_request).unwrap();
    let mut report_reader = BufReader::new(report_stream);
    let mut report_response = String::new();
    report_reader.read_line(&mut report_response).unwrap();

    // Read messages from the client stream and look for the semantic notification.
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut found_notify = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match read_server_message(&mut stream) {
            Ok((variant, _payload)) => {
                if variant == SERVER_MESSAGE_SEMANTIC_NOTIFICATION {
                    found_notify = true;
                    break;
                }
                // Snapshot and pane-surface messages may arrive first.
            }
            Err(_) => {
                break;
            }
        }
    }

    assert!(
        found_notify,
        "client should receive a semantic notification after pane.report_agent"
    );

    // Now report Idle from Working — this should trigger a Done sound
    // if the pane is in a background workspace.
    // First, create a second workspace to make the first one "background".
    let mut ws2_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let ws2_request = r#"{"id":"4","method":"workspace.create","params":{}}"#;
    writeln!(ws2_stream, "{}", ws2_request).unwrap();
    let mut ws2_reader = BufReader::new(ws2_stream);
    let mut ws2_response = String::new();
    ws2_reader.read_line(&mut ws2_response).unwrap();

    // Focus the new workspace (making the first one background).
    let ws2_id = ws2_response
        .split('"')
        .find(|s| s.starts_with("w_"))
        .unwrap_or("w_2")
        .to_string();
    let mut focus_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let focus_request = format!(
        r#"{{"id":"5","method":"workspace.focus","params":{{"workspace_id":"{ws2_id}"}}}}"#
    );
    writeln!(focus_stream, "{}", focus_request).unwrap();
    let mut focus_reader = BufReader::new(focus_stream);
    let mut focus_response = String::new();
    focus_reader.read_line(&mut focus_response).unwrap();

    assert!(
        wait_until(Duration::from_secs(2), Duration::from_millis(25), || {
            ping_socket(&api_socket).contains("pong")
        }),
        "server should stay responsive after workspace focus"
    );

    // Report agent as Working first, then Idle — this transition in a
    // background workspace should trigger a Done sound notification.
    let mut work_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let work_request = format!(
        r#"{{"id":"6","method":"pane.report_agent","params":{{"pane_id":"{pane_id}","agent":"pi","state":"working","source":"test"}}}}"#
    );
    writeln!(work_stream, "{}", work_request).unwrap();
    let mut work_reader = BufReader::new(work_stream);
    let mut work_response = String::new();
    work_reader.read_line(&mut work_response).unwrap();

    assert!(
        wait_until(Duration::from_secs(2), Duration::from_millis(25), || {
            ping_socket(&api_socket).contains("pong")
        }),
        "server should stay responsive after working state report"
    );

    let mut idle_stream = UnixStream::connect(&api_socket).expect("connect to API");
    let idle_request = format!(
        r#"{{"id":"7","method":"pane.report_agent","params":{{"pane_id":"{pane_id}","agent":"pi","state":"idle","source":"test"}}}}"#
    );
    writeln!(idle_stream, "{}", idle_request).unwrap();
    let mut idle_reader = BufReader::new(idle_stream);
    let mut idle_response = String::new();
    idle_reader.read_line(&mut idle_response).unwrap();

    // Read messages and look for the done semantic notification.
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut found_done_notify = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match read_server_message(&mut stream) {
            Ok((variant, _payload)) => {
                if variant == SERVER_MESSAGE_SEMANTIC_NOTIFICATION {
                    found_done_notify = true;
                    break;
                }
                // Snapshot and pane-surface messages may arrive first.
            }
            Err(e) => {
                eprintln!("read error while looking for done notification: {e}");
                break;
            }
        }
    }

    assert!(
        found_done_notify,
        "client should receive a semantic notification when a background pane transitions Working→Idle"
    );

    cleanup_spawned_herdr(spawned, base);
}
