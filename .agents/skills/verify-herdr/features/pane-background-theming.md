# Pane background theming

Pane background theming assigns separate colors to default-background terminal content and Herdr-owned pane chrome.

## Sub-features

- `pane-background-border` uses edge-aligned vertical glyphs and cached horizontal pixel strips in Ghostty, preserving green focused and gray unfocused colors. Text borders remain the fallback.
- `pane-background-interior` applies `pane_default_bg` to terminal content and its scrollbar lane where an application has not drawn a background.
- `pane-background-streaming` applies that same default before presenting each incremental text or scrollbar update.
- `pane-background-popup` applies the same default to popup terminals.
- `pane-background-gap` colors exterior gutters and temporary resize slack. Pixel masks also carve the stacked gutter out of the reserved horizontal border rows.
- `pane-background-slack` colors canvas cells that a retained pane surface does not own.
- `pane-background-reset` leaves all cells unchanged when the token is `reset`.

## User path

- Set `[theme.custom] pane_gap_bg = "#221f22"` and `pane_default_bg = "#1e1e2e"` in an isolated Herdr configuration.
- Open multiple bordered panes.
- Resize the client or pane layout and inspect the area outside the frames.

## Drive it

Run `verify-herdr.sh pane-backgrounds <evidence-directory>` after doctor passes.

The helper first composes a retained pane surface with deliberate bottom and right slack. It then starts an isolated server and a real 80 by 24 client PTY, creates one right split and one down split, and waits for all three pane markers.

`census.txt` must report 1,574 pane-body cells including scrollbar lanes. Of these, 1,563 use the pane default and 11 use an application-defined true-color background. All 322 border cells use the pane default. All 24 exterior gutter cells use the gap background. The test checks vertical glyphs, cleared horizontal glyphs, and focus colors. These counts describe the text layer; the pixel strips draw over its horizontal border cells.

A second PTY run opens a 20-column framed sidebar after one blank left column. Its content is inset by one cell on every edge, and one blank right gutter separates it from the pane canvas. The `sidebar/` evidence reports 1,112 body cells, 256 border cells, and 24 exterior cells within the pane canvas. It clicks a real workspace row, hides and reveals the sidebar, resizes it to 24 columns, and checks that hidden chrome retires its images. `interaction.ansi` keeps the complete upload and cleanup history.

Both PTYs advertise exact 17 by 36 pixel cells and Ghostty graphics support. The sidebar run decodes two sidebar strips with the sidebar interior color plus all six pane strips. The hidden-sidebar run decodes the six pane strips. Each check covers every pixel. Top strips place 17 exterior pixels above the two-pixel stroke. Bottom strips place no exterior pixels below the stroke. The adjacent margins still make a 17-pixel stacked gutter. The test also rejects repeated image uploads during text updates.

A separate named-pane PTY uses a CJK label that contains a border glyph and a second ASCII label. It checks the literal text cells and decodes all four border strips. Each title chip fills its padded cell range from pixel row 4 through row 31, uses the rendered border color, and leaves the neighboring exterior and interior pixels unchanged. The text renderer still owns truncation, glyph placement, contrast, and focus styling. The pixel layer only adds a vertically centered fill behind that text. Replay `named-panes/raw.ansi` in the native terminal and inspect the chip beside the text references before accepting a screenshot.

The test enables scrollbars and exercises both visible and hidden lanes. It then sends three text updates and checks every completed output frame. `streaming.ansi` records those updates. `visual.log` must report at least three streaming frames without a background mismatch.

## Invalid proof

- A split drag hit rectangle includes border cells and does not define styling ownership.
- A palette unit test does not prove the client rendered the background. A cell census does not locate the stroke within a cell. Use the [native terminal recipe](terminal-rendering.md) for that check.
- To color host pixels below or beside the cell grid, set the terminal emulator's window background to the gap color and its padding color to that background. Herdr cannot draw outside the grid.
- With Ghostty, use `background = #221f22` and `window-padding-color = background`. Do not use `extend` or `extend-always`, which copy neighboring cell backgrounds.
- `pane_default_bg` changes the displayed default inside Herdr panes. A child process that queries OSC 11 still receives the terminal emulator's global background.
- A run against the user's default socket is invalid.
