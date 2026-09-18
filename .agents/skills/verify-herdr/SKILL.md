---
name: verify-herdr
description: Verify Herdr's terminal UI through isolated real server and client processes, a PTY, and libghostty-vt screen inspection. Use for pane layout, theme boundaries, client lifecycle, and other visible TUI changes.
---

# Verify Herdr

Drive the checkout build in isolated Unix sockets and PTYs. Never address the user's running Herdr session.

## Launch

Run from the repository root. Set `ZIG` to a Zig 0.16.0 binary when that version is not first on `PATH`.

```bash
ZIG=/path/to/zig-0.16.0 .agents/skills/verify-herdr/scripts/verify-herdr.sh pane-backgrounds
```

The harness builds `target/debug/herdr`, creates a unique temporary config/runtime directory, starts a real Herdr server, creates and splits panes through the socket API, and attaches a real client in an 80×24 PTY. Readiness means all three pane markers are present in the last completed synchronized-output frame, not a partial byte stream.

The integration test owns its child PIDs and temporary sockets. Its drop guards stop only those children and remove only its unique test directory.

## Doctor

Run this read-only check before driving a feature:

```bash
.agents/skills/verify-herdr/scripts/verify-herdr.sh doctor
```

Doctor requires the Herdr checkout, Cargo, and Zig 0.16.0. A failure means the instance is not worth driving; correct the reported local prerequisite first.

## Drive

Read [features/README.md](features/README.md), then follow the matching feature recipe. For the pane background boundary:

```bash
ZIG=/path/to/zig-0.16.0 .agents/skills/verify-herdr/scripts/verify-herdr.sh pane-backgrounds /private/tmp/herdr-pane-background-proof
```

The unit checks cover Reset-only pane filling, unowned canvas cells, explicit application backgrounds, and popup terminals. The integration check uses a real client PTY with three bordered panes and replays its ANSI output through the same libghostty-vt terminal engine Herdr uses.

For broader agent or application reproductions, use the sibling `herdr-throwaway-repro` skill. It creates a named disposable session through the installed CLI; keep that workflow separate from this checkout-build verifier.

## Capture the native render

On macOS, use the managed capture helper. It requires Ghostty, Python 3.11 or later, and the Xcode command-line tools:

```bash
python3 .agents/skills/verify-herdr/scripts/capture_ghostty.py \
  /private/tmp/herdr-pane-background-proof/sidebar/raw.ansi \
  /private/tmp/herdr-pane-background-proof/sidebar/native.png
```

The helper records its Ghostty window and terminal IDs, captures that window, and verifies that the window is absent before it returns. It refuses to close a window that contains an unowned terminal. Inspect the PNG yourself and check that the recorded grid fits without cropping. Capture readiness does not prove visual correctness.

## Evidence

The helper records the command output plus these real-user-path artifacts in the requested evidence directory:

- `raw.ansi` — bytes emitted by the real Herdr client.
- `screen.txt` — the reconstructed 80×24 terminal screen.
- `config.toml` — the isolated configuration used by the run.
- `census.txt` — terminal interior, pane border, and unowned-canvas color counts.
- `unit.log` and `visual.log` — action and result logs with exit status enforced by the helper.
- `named-panes/raw.ansi`, `named-panes/screen.txt`, and `named-panes/config.toml` — the two-pane Unicode title proof for native replay.
- The native PNG and its adjacent `.png.json` ownership and cleanup record.

The pane-background proof checks pane bodies, visible and hidden scrollbar lanes, border cells, and gaps. It fixes host cells at 17 by 36 pixels, decodes six PNG border strips, and checks two-pixel strokes with a 17-pixel top margin and zero-pixel bottom margin. The stacked gutter remains 17 pixels. Use [terminal rendering](features/terminal-rendering.md) for native measurements, color-space checks, and replay; the text-cell census alone cannot prove pixel geometry. It also inspects each completed frame during text updates. Default pane cells must retain the configured pane background from their first render, and application-defined colors must stay unchanged. See the feature recipe for the expected cell census.

This proof starts a fresh server. For an installed build, also verify that the running server loaded the new executable. Detaching and relaunching a client does not replace the server or apply server-side layout changes. Keep any authorized live update separate from the isolated verifier and confirm that pane processes survive it.

## Cleanup

Cleanup is automatic even on test failure. The test harness terminates only the PIDs it registered, removes its unique `/tmp/herdr-client-test-*` runtime, and never connects to the default Herdr socket. The requested evidence directory is outside that runtime and survives cleanup.

The native capture helper closes its exact window after success, capture failure, SIGINT, and SIGTERM. Require its JSON `cleanup_status` to say `closed and absent` or `already absent`. Replay PID exit is not proof of window cleanup. The helper fails and reports the owned IDs if cleanup fails.

SIGKILL and host crashes cannot run `finally`. Recover only from the recorded ownership IDs after inspecting the exact window. Never close by title, quit Ghostty, kill by process name, or send input to another terminal.

If a test process is interrupted before Rust drop guards run, use the repository test support's PID/runtime ownership markers; do not kill processes by name and do not stop the user's default session.

## Helpers

`.agents/skills/verify-herdr/scripts/verify-herdr.sh` supports:

```text
verify-herdr.sh doctor
verify-herdr.sh pane-backgrounds [evidence-directory]
```

The helper is safe to rerun. Use a new evidence directory when retaining more than one proof.
