# Verify terminal border geometry

Use this recipe when a screenshot shows a color strip or uneven gap that a cell-background test does not explain. Keep the isolated PTY test and the native terminal capture as separate evidence.

## Measure before editing

1. Save the original screenshot at its native resolution. Do not measure a scaled preview.
2. Choose a row that crosses the sidebar separator and pane borders without text. Choose a column that crosses a stacked split.
3. Run the pixel scanner with those coordinates. Pillow is required only for screenshot measurement.

```bash
python3 /path/to/verify-herdr/scripts/pixel_runs.py screenshot.png \
  --row 870 --start 427 --end 480
python3 /path/to/verify-herdr/scripts/pixel_runs.py screenshot.png \
  --column 1600 --start 1600 --end 1700
```

The output gives contiguous RGB runs and their pixel widths. End coordinates are exclusive. Identify colors from flat areas in the same capture. A screenshot's color profile can change RGB values, so do not compare unconverted screenshot values directly with configured hex colors.

Count each owner separately: pane content, scrollbar lane, border stroke, unused pixels within the border cell, and exterior gutter. A centered box-drawing glyph leaves roughly half of its cell on each side of its stroke. Painting the cell's background colors both halves. Repainting a smaller pane-body rectangle cannot change that split.

In the September 2026 Ghostty reproduction, cells were 17 by 36 pixels. The sidebar gap measured 15 pixels and the split gap measured 32 pixels. Monokai occupied another 7 or 8 pixels inside each vertical border. The corrected outer-edge frames had 17-pixel horizontal gutters at both locations and no inner Monokai strip.

## Test representations in the host terminal

Render the probe in a managed Ghostty window at least 80 columns by 26 rows. Do not send it into an agent pane or the user's attached Herdr client.

```bash
python3 /path/to/verify-herdr/scripts/border_probe.py > /tmp/herdr-border-probe.ansi
python3 /path/to/verify-herdr/scripts/capture_ghostty.py \
  /tmp/herdr-border-probe.ansi /tmp/herdr-border-probe.png
```

Inspect the PNG's corners and straight edges. Require its adjacent `.png.json` record to report `closed and absent` or `already absent` for `cleanup_status`.

| Candidate | What to check |
| --- | --- |
| Centered box drawing | Background strips on both sides of the stroke |
| Outer-edge eighth blocks | Mocha reaches the inside of every stroke; corners join |
| Inner-edge eighth blocks | Corner extensions and doubled exterior gutter width |
| Underline and overline | Actual stroke position, thickness, and color support |
| Shared divider | Focus ownership and loss of a separate Monokai gutter |

Ghostty 1.3.1 rendered the outer-edge set `🭽▔🭾`, `▏ ▕`, `🭼▁🭿` correctly. Colored underlines and overlines also rendered. That result does not establish support in every terminal or font. Test the target terminal before selecting a representation or declaring a pixel split impossible.

Outer-edge blocks remove the inside strip, but one blank column and one blank row remain unequal in pixels. Horizontal eighth-block strokes are also thicker. Do not claim those glyphs alone can satisfy equal pixel spacing in both axes.

## Verify pixel strips

For the Ghostty pixel-frame path, bordered stacked panes reserve no blank row. Each top or bottom border row carries a cached PNG strip behind its text. At 17 by 36 pixels per cell, top strips contain 17 exterior pixels followed by a two-pixel stroke. Bottom strips place the two-pixel stroke at rows 34 and 35 with no exterior pixels below it. Adjacent margins still sum to 17 pixels, matching the blank column between side-by-side panes.

Named panes keep their labels in the text layer, so the existing layout still controls Unicode width, truncation, contrast, focus styling, and glyph positions. The PNG adds a fill behind the padded title span from row 4 through row 31. This centers the chip vertically while preserving the exterior pixels above it and the pane interior below it.

The PTY recipe sets the host pixel dimensions explicitly. It decodes all six background-proof PNGs and all four named-pane PNGs, checks their complete pixel arrays, and confirms that streaming text reuses uploads. The named run checks a real CJK label with a border glyph at literal cell positions. Replay `named-panes/raw.ansi` in Ghostty for the native proof, then compare the chip fill and text against the PTY's decoded pixels. Readiness must use the last completed synchronized-output frame (`CSI ? 2026 l`): a pane marker can arrive while the border or graphics bytes are still being written.

Capture the actual host render as well. Check two-pixel strokes, 17-pixel gutters in both directions, joined corners, unchanged application backgrounds, and no shade change where an image meets a text cell. Check labels, occlusion, focus changes, and workspace cleanup. Profile cached redraws with one and fifteen panes; PNG compression and placement encoding must not repeat on every text update.

Ghostty/macOS's default sRGB text path converts colors to Display P3, while its Metal image path uses that output space directly. The image encoder must account for this difference. In the tested configuration, the native Mocha, Monokai, green, and gray pixels matched adjacent text-rendered references after conversion. A mismatch between image and text colors in one capture is not merely a screenshot-profile caveat. Non-default host color spaces need a separate native check. Other terminals and graphics-disabled clients retain the text fallback.

## Replay the real client capture

After `pane-backgrounds` passes, capture its bytes in a managed host window:

```bash
python3 /path/to/verify-herdr/scripts/capture_ghostty.py \
  /tmp/herdr-pane-background-proof/sidebar/raw.ansi \
  /tmp/herdr-pane-background-proof/sidebar/native.png
```

Use only captures from the isolated verifier. ANSI can include terminal commands, not just text. Keep the host grid at least as large as the recorded grid and inspect the PNG for cropping. The helper does not resize the host window. Its replay child enters raw mode so terminal query responses do not echo into the picture.

The helper resolves the numeric Core Graphics window ID and calls `screencapture -x -o -l` on that window. Ghostty's AppleScript window ID is not interchangeable. A full-desktop capture can show the lock screen instead of the preview. Open the result and confirm it contains the fixture before measuring it. Direct window captures can use a different color profile, so compare image and text colors within the same capture.

The helper closes its exact window in `finally`, including after capture failure, SIGINT, and SIGTERM. Require the JSON ownership record to confirm that the window is absent. Neither replay PID exit nor a manual `q` proves window cleanup. If cleanup fails, report the recorded IDs and inspect that exact window. Never close by title or send input to an unowned terminal. SIGKILL and host crashes require recovery from those IDs.

A capture taken after a focus change or resize can contain only image placements. A fresh terminal also needs the earlier image uploads. For `line_tabs_keep_panel_background_and_follow_focus`, replay `tabs/replay/raw.ansi`, which includes the initial uploads and subsequent updates. The individual `narrow`, `focused-second`, and `wide` captures are frame updates, not standalone sessions.

Measure the sidebar gutter, the side-by-side gutter, and the stacked gutter again. Check the pane's first pixel after the border, its scrollbar lane, and focused and unfocused corners. A passing libghostty-vt cell census alone cannot prove where the host renderer places a stroke.

## Check an explicitly authorized installed update

The isolated verifier never updates the user's session. If the user separately requests installation and live handoff, keep its evidence in a separate directory.

1. Record the installed binary hash, `herdr status --json`, and `herdr api snapshot`.
2. For each pane ID, record `herdr pane process-info --pane <id>`. Record OS process start times as protection against PID reuse.
3. Install the verified binary atomically. Run only the live-handoff command authorized by the user, with the expected version and protocol. Never use `herdr server stop` for this workflow.
4. Check the new server's status and `server_binary_stale`. Relaunch the attached client after its old process exits so client-side composition changes load too. Target the known host terminal, not a pane running an agent. Before sending any input, check that its foreground process is the expected shell. If a new Herdr client is already running on that TTY, skip the relaunch. Do not run a process check and then send input unconditionally. Clear any pending shell input before pasting a command; otherwise a restored command line can concatenate two executable paths.
5. Compare pane IDs, shell PIDs, foreground process groups, start times, and process liveness. Report any differences. In the tested handoff, runtime terminal IDs changed while all ten pane IDs and their processes survived; terminal ID equality was not a valid process-survival check.
6. Restore the user's selected workspace if reattachment changed it. Capture the installed UI and retain the before and after evidence.

Do not automate installation or handoff merely because a visual test passed.
