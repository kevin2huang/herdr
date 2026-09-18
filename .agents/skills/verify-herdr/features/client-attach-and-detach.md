# Client attach and detach

Client attach renders the current server state in a real terminal, and detach restores outer-terminal modes without stopping the server.

## Sub-features

- `client-attach` receives a coherent snapshot and pane surface.
- `client-detach` restores mouse, paste, cursor, and screen modes.
- `client-reattach` renders output produced while no client was attached.
- `server-survives-detach` keeps the session runtime available.

## How to get to it (user POV)

- Run `herdr` or `herdr client` to attach.
- Press the configured prefix followed by `q` to detach.
- Run the client again to reattach.

## Driving it with verify-herdr

Preconditions:

- Use the checkout build and isolated API/client sockets.
- Capture PTY output from before attach through terminal restoration.

- **Attach.** Run `cargo test --locked --test client_mode client_shell_detaches_restores_and_freshly_reattaches_to_current_state -- --exact --nocapture --test-threads=1` with `ZIG` set.
- **Detach.** The test sends the real prefix/detach key sequence and requires terminal teardown sequences.
- **Reattach.** It writes a marker while detached, starts a fresh client, and requires that marker on the reconstructed screen.
- **Proof.** Retain the command output and exit code outside the test runtime.

## Gotchas

- Closing the PTY is not equivalent to the user's detach key path.
- Do not interpret a server process exit as successful client cleanup.
- Partial ANSI redraws must be replayed into a terminal screen before text assertions.
- Never attach this lifecycle test to the user's live server.
