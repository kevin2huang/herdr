# Pane splitting and layout

Pane splitting creates right or down terminal panes, preserves their identities, and recomputes their rectangles for the attached client size.

## Sub-features

- `pane-split-right` creates a horizontal BSP split.
- `pane-split-down` creates a vertical BSP split.
- `pane-layout-rects` exposes the resulting layout through the pane API.
- `pane-focus` chooses which pane receives terminal input.

## User path

- Right-click a pane frame and choose a split direction.
- Run `herdr pane split <pane-id> --direction right|down`.
- Drag a split frame to resize it.

## Drive it

Use a unique socket and runtime directory. Address only pane IDs returned by `workspace.create` and `pane.split`.

The pane-background recipe creates a workspace, splits its root pane right at `0.5`, and splits the returned right pane down at `0.5`. With borders and pane gaps enabled, an 80 by 24 hidden-chrome client produces these outer rectangles:

```text
left          0,0 39x24
top-right    40,0 40x12
bottom-right 40,12 40x12
```

`screen.txt` must contain `LEFT_READY`, `TOP_RIGHT_READY`, and `BOTTOM_RIGHT_READY` inside the three frames.

Side-by-side panes have one blank column. Bordered stacked panes reserve no blank row. In Ghostty, pixel strips use the existing top and bottom border rows to provide a vertical gutter equal to one cell's width: 17 pixels for the verifier's 17 by 36 cells. The glyph-only fallback cannot reproduce that sub-cell spacing.

## Invalid proof

- Server fallback geometry does not prove attached-client geometry.
- A pane's `inner_rect` excludes its border lane. Its `rect` includes the border.
- Fixed sleeps do not prove terminal readiness.
- Cleanup that targets an unregistered process or a shared runtime is unsafe.
