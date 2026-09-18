# Workspace and tab navigation

Workspace and tab navigation lets a user select session groups from the sidebar and tabs from the tab bar while the active pane surface follows the selection.

## Sub-features

- `workspace-select` changes the active workspace from the sidebar.
- `tab-select` changes the active tab from the tab strip.
- `active-highlight` shows the selected row and tab with theme colors.
- `tab-overflow` keeps the focused tab reachable in a narrow client.

## How to get to it (user POV)

- Click a workspace in the left sidebar.
- Click a numbered tab in the tab bar.
- Enter Navigate mode and move between workspace, tab, and pane targets.

## Driving it with verify-herdr

Preconditions:

- Use an isolated server with at least two workspaces or tabs.
- Keep sidebar and tab chrome visible for this feature.

- **Tab overflow.** Run `cargo test --locked --bin herdr client::shell::tests::chrome_context::tab_overflow_controls_scroll_the_client_owned_tab_bar -- --exact --nocapture` with `ZIG` set.
- **Workspace selection.** Use the isolated PTY and the rendered sidebar label to derive a mouse row; do not use a hard-coded coordinate.
- **Tab selection.** Select the tab through its rendered label and require the corresponding pane marker to appear.
- **Proof.** Capture action bytes, reconstructed screen text, and the selected pane marker.

## Gotchas

- Hidden or compact sidebar modes change the user entry point.
- Tab width and overflow depend on the client width and mouse-capture setting.
- A palette unit test does not prove the highlighted cell was rendered.
- Do not use the default session merely because it already has multiple tabs.
