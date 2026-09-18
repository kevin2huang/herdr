use std::{mem::size_of, ptr};

// Reuse the generated C API; this test observer needs only terminal parsing and formatting.
#[allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]
#[path = "../../src/ghostty/bindings.rs"]
mod ffi;

struct Screen {
    terminal: ffi::GhosttyTerminal,
    formatter: ffi::GhosttyFormatter,
}

impl Drop for Screen {
    fn drop(&mut self) {
        unsafe {
            ffi::ghostty_formatter_free(self.formatter);
            ffi::ghostty_terminal_free(self.terminal);
        }
    }
}

struct RenderScreen {
    terminal: ffi::GhosttyTerminal,
    state: ffi::GhosttyRenderState,
    rows: ffi::GhosttyRenderStateRowIterator,
    cells: ffi::GhosttyRenderStateRowCells,
}

impl Drop for RenderScreen {
    fn drop(&mut self) {
        unsafe {
            ffi::ghostty_render_state_row_cells_free(self.cells);
            ffi::ghostty_render_state_row_iterator_free(self.rows);
            ffi::ghostty_render_state_free(self.state);
            ffi::ghostty_terminal_free(self.terminal);
        }
    }
}

pub fn text(output: &[u8], cols: u16, rows: u16) -> String {
    let mut screen = Screen {
        terminal: ptr::null_mut(),
        formatter: ptr::null_mut(),
    };
    let options = ffi::GhosttyFormatterTerminalOptions {
        size: size_of::<ffi::GhosttyFormatterTerminalOptions>(),
        emit: ffi::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_PLAIN,
        trim: true,
        extra: ffi::GhosttyFormatterTerminalExtra {
            size: size_of::<ffi::GhosttyFormatterTerminalExtra>(),
            screen: ffi::GhosttyFormatterScreenExtra {
                size: size_of::<ffi::GhosttyFormatterScreenExtra>(),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe {
        assert_eq!(
            ffi::ghostty_terminal_new(ptr::null(), &mut screen.terminal, cols, rows),
            ffi::GhosttyResult_GHOSTTY_SUCCESS
        );
        ffi::ghostty_terminal_vt_write(screen.terminal, output.as_ptr(), output.len());
        assert_eq!(
            ffi::ghostty_formatter_terminal_new(
                ptr::null(),
                &mut screen.formatter,
                screen.terminal,
                options,
            ),
            ffi::GhosttyResult_GHOSTTY_SUCCESS
        );
        let mut len = 0;
        let result =
            ffi::ghostty_formatter_format_buf(screen.formatter, ptr::null_mut(), 0, &mut len);
        assert!(matches!(
            result,
            ffi::GhosttyResult_GHOSTTY_SUCCESS | ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE
        ));
        let mut bytes = vec![0; len];
        assert_eq!(
            ffi::ghostty_formatter_format_buf(
                screen.formatter,
                bytes.as_mut_ptr(),
                bytes.len(),
                &mut len
            ),
            ffi::GhosttyResult_GHOSTTY_SUCCESS
        );
        String::from_utf8_lossy(&bytes[..len]).into_owned()
    }
}

pub fn explicit_backgrounds(output: &[u8], cols: u16, rows: u16) -> Vec<Option<[u8; 3]>> {
    explicit_colors(
        output,
        cols,
        rows,
        ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
    )
}

pub fn explicit_foregrounds(output: &[u8], cols: u16, rows: u16) -> Vec<Option<[u8; 3]>> {
    explicit_colors(
        output,
        cols,
        rows,
        ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
    )
}

fn explicit_colors(
    output: &[u8],
    cols: u16,
    rows: u16,
    data: ffi::GhosttyRenderStateRowCellsData,
) -> Vec<Option<[u8; 3]>> {
    let mut screen = RenderScreen {
        terminal: ptr::null_mut(),
        state: ptr::null_mut(),
        rows: ptr::null_mut(),
        cells: ptr::null_mut(),
    };
    unsafe {
        assert_eq!(
            ffi::ghostty_terminal_new(ptr::null(), &mut screen.terminal, cols, rows),
            ffi::GhosttyResult_GHOSTTY_SUCCESS
        );
        ffi::ghostty_terminal_vt_write(screen.terminal, output.as_ptr(), output.len());
        assert_eq!(
            ffi::ghostty_render_state_new(ptr::null(), &mut screen.state),
            ffi::GhosttyResult_GHOSTTY_SUCCESS
        );
        assert_eq!(
            ffi::ghostty_render_state_update(screen.state, screen.terminal),
            ffi::GhosttyResult_GHOSTTY_SUCCESS
        );
        assert_eq!(
            ffi::ghostty_render_state_row_iterator_new(ptr::null(), &mut screen.rows),
            ffi::GhosttyResult_GHOSTTY_SUCCESS
        );
        assert_eq!(
            ffi::ghostty_render_state_get(
                screen.state,
                ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                (&mut screen.rows as *mut ffi::GhosttyRenderStateRowIterator).cast(),
            ),
            ffi::GhosttyResult_GHOSTTY_SUCCESS
        );
        assert_eq!(
            ffi::ghostty_render_state_row_cells_new(ptr::null(), &mut screen.cells),
            ffi::GhosttyResult_GHOSTTY_SUCCESS
        );
    }

    let mut backgrounds = Vec::with_capacity(usize::from(cols) * usize::from(rows));
    for _ in 0..rows {
        assert!(unsafe { ffi::ghostty_render_state_row_iterator_next(screen.rows) });
        assert_eq!(
            unsafe {
                ffi::ghostty_render_state_row_get(
                    screen.rows,
                    ffi::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                    (&mut screen.cells as *mut ffi::GhosttyRenderStateRowCells).cast(),
                )
            },
            ffi::GhosttyResult_GHOSTTY_SUCCESS
        );
        for _ in 0..cols {
            assert!(unsafe { ffi::ghostty_render_state_row_cells_next(screen.cells) });
            let mut color = ffi::GhosttyColorRgb::default();
            let result = unsafe {
                ffi::ghostty_render_state_row_cells_get(
                    screen.cells,
                    data,
                    (&mut color as *mut ffi::GhosttyColorRgb).cast(),
                )
            };
            backgrounds.push(match result {
                ffi::GhosttyResult_GHOSTTY_SUCCESS => Some([color.r, color.g, color.b]),
                ffi::GhosttyResult_GHOSTTY_INVALID_VALUE => None,
                other => panic!("unexpected background query result: {other}"),
            });
        }
    }
    backgrounds
}

#[test]
fn screen_text_reconstructs_partial_redraws() {
    let output = b"REMOTE_SURVIVED\x1b[1;8HSTILL_SELECTED";
    assert!(!output
        .windows(b"REMOTE_STILL_SELECTED".len())
        .any(|part| part == b"REMOTE_STILL_SELECTED"));
    assert!(text(output, 80, 24).contains("REMOTE_STILL_SELECTED"));
}

#[test]
fn screen_backgrounds_distinguish_explicit_rgb_from_terminal_default() {
    let output = b"\x1b[48;2;34;31;34mA\x1b[49mB";
    let backgrounds = explicit_backgrounds(output, 2, 1);
    assert_eq!(backgrounds, vec![Some([34, 31, 34]), None]);
}
