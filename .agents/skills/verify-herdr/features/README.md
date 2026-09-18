# Herdr verification map

This directory maps Herdr's primary user-facing terminal workflows to isolated verification recipes.

## Baseline preconditions

- Run from the Herdr repository root.
- Set `ZIG` to Zig 0.16.0.
- Run `../scripts/verify-herdr.sh doctor` before driving a feature.
- Use checkout-built binaries, unique runtime directories, and explicit socket paths.
- Never drive or stop the user's default Herdr session.

## Driving conventions

- Prefer the real CLI/socket command and a client PTY over internal state setters.
- Wait for pane text or an API response instead of sleeping for a guessed duration.
- Record the action, reconstructed terminal screen, cell-color census, and command output.
- The terminal `inner_rect` excludes its scrollbar lane. The pane owns its scrollbar lane and edge-aligned border cells. Only cells outside the pane rectangle belong to the canvas. Horizontal pixel masks refine that text-cell boundary: measure the visible gap separately.
- Keep proof artifacts outside the disposable runtime directory.

## Proof and skip reporting

- A visible assertion requires a real client render, not only a palette/config parse.
- A layout assertion includes the pane IDs and rectangles returned through the user-facing API.
- A lifecycle assertion proves both the visible transition and process/socket cleanup.
- Report an unreachable path with the failed command and prerequisite; do not substitute the default session.

## Feature entry contract

Each feature file gives the user entry point, exact harness action, observable result, and traps that invalidate the proof.

## Features

- [Terminal rendering](./terminal-rendering.md) covers native measurements, pixel strips, color spaces, ANSI replay, and separately authorized live updates.
- [Pane background theming](./pane-background-theming.md) covers pane frames, terminal content, and the outside canvas.
- [Pane splitting and layout](./pane-splitting-and-layout.md) covers right/down splits and stable pane geometry.
- [Client attach and detach](./client-attach-and-detach.md) covers terminal restoration and reattachment.
- [Workspace and tab navigation](./workspace-and-tab-navigation.md) covers the visible sidebar/tab selection path.
