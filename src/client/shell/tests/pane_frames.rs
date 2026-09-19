use super::*;
use crate::client::shell::render::put_text;
use ratatui::widgets::{Block, Borders, Widget};

fn fixture(count: usize) -> (FrameData, Vec<PaneSurfacePane>, Palette) {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 120, 40));
    let mut panes = Vec::new();
    let palette = Palette {
        accent: Color::Rgb(169, 220, 118),
        overlay0: Color::Rgb(91, 89, 92),
        pane_default_bg: Color::Rgb(30, 30, 46),
        pane_gap_bg: Color::Rgb(34, 31, 34),
        ..Palette::catppuccin()
    };
    for index in 0..count {
        let rect = if count == 1 {
            buffer.area
        } else {
            Rect::new((index % 5) as u16 * 24, (index / 5) as u16 * 13, 23, 13)
        };
        let stroke = if index == 0 {
            palette.accent
        } else {
            palette.overlay0
        };
        let title_fg = if index == 0 {
            Color::Rgb(0, 0, 0)
        } else {
            Color::Rgb(255, 255, 255)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(crate::ui::PANE_BORDER_SET)
            .border_style(Style::default().fg(stroke))
            .title_style(Style::default().fg(title_fg).bg(stroke))
            .title(" label ");
        let inner = block.inner(rect);
        block.render(rect, &mut buffer);
        panes.push(PaneSurfacePane {
            pane_id: format!("pane-{index}"),
            content_revision: 0,
            rect: rect.into(),
            inner_rect: inner.into(),
            scrollbar_rect: None,
            scroll: None,
            focused: index == 0,
            mouse_reporting: false,
            sgr_pixel_mouse: false,
            alternate_screen_active: false,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
    (
        FrameData::from_ratatui_buffer(&buffer, None),
        panes,
        palette,
    )
}

const CELL: HostCellSize = HostCellSize {
    width_px: 17,
    height_px: 36,
};

fn layout(panes: &[PaneSurfacePane], active_tab: Option<Rect>) -> ChromeLayout<'_> {
    ChromeLayout {
        panes,
        pane_area: Rect::default(),
        sidebar: None,
        active_tab,
        icons: &[],
    }
}

fn tab_frame() -> FrameData {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 20, 2));
    put_text(
        &mut buffer,
        2,
        0,
        8,
        " First  ",
        Style::default()
            .fg(Color::Rgb(169, 220, 118))
            .bg(Color::Rgb(34, 31, 34))
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    );
    FrameData::from_ratatui_buffer(&buffer, None)
}

fn assert_font_underline_fallback(
    active_tab: Option<Rect>,
    cell: HostCellSize,
    palette: &Palette,
    occlusion: &surface::Occlusion,
) {
    let mut frames = PaneFrames::default();
    frames.set_scope("fallback");
    let mut frame = tab_frame();
    let bytes = frames.compose(
        &mut frame,
        layout(&[], active_tab),
        cell,
        palette,
        occlusion,
    );
    assert!(!bytes.windows(5).any(|bytes| bytes == b"a=t,t"));
    assert!(frame.cells[2..10]
        .iter()
        .all(|cell| cell.modifier & Modifier::UNDERLINED.bits() != 0));
}

#[test]
fn tab_underline_png_uses_only_the_bottom_two_pixel_rows() {
    let underline = TabUnderline {
        rect: Rect::new(0, 0, 1, 1),
        cell_width: 17,
        cell_height: 36,
        accent: [169, 220, 118],
    };
    let mut reader = png::Decoder::new(std::io::Cursor::new(underline.png().unwrap()))
        .read_info()
        .unwrap();
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert_eq!((info.width, info.height), (17, 36));
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!(pixels[..17 * 34 * 4], vec![0; 17 * 34 * 4]);
    let accent = if cfg!(target_os = "macos") {
        [179, 219, 130, 255]
    } else {
        [169, 220, 118, 255]
    };
    assert_eq!(pixels[17 * 34 * 4..], accent.repeat(17 * 2));
    assert!(TabUnderline {
        rect: Rect::new(0, 0, u16::MAX, 1),
        cell_width: 17,
        cell_height: 36,
        accent: [169, 220, 118],
    }
    .png()
    .is_err());
}

fn icon_row(icon: crate::ui::SidebarIcon, color: Option<[u8; 3]>) -> IconRow {
    IconRow {
        placement: SidebarIconPlacement {
            icon,
            rect: Rect::new(0, 0, 2, 1),
        },
        cell_width: CELL.width_px,
        cell_height: CELL.height_px,
        color,
    }
}

fn decode_icon(row: IconRow) -> (png::OutputInfo, Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(row.png().unwrap()))
        .read_info()
        .unwrap();
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut pixels).unwrap();
    pixels.truncate(info.buffer_size());
    (info, pixels)
}

#[test]
fn sidebar_svg_raster_preserves_dimensions_viewport_colors_and_straight_alpha() {
    use crate::ui::SidebarIcon;

    for icon in [
        SidebarIcon::GitBranch,
        SidebarIcon::Pi,
        SidebarIcon::Claude,
        SidebarIcon::Codex,
    ] {
        let color = (icon == SidebarIcon::GitBranch).then_some([12, 34, 56]);
        let (info, pixels) = decode_icon(icon_row(icon, color));
        assert_eq!((info.width, info.height), (34, 36));
        assert_eq!(info.color_type, png::ColorType::Rgba);
        for (index, pixel) in pixels.chunks_exact(4).enumerate() {
            if pixel[3] == 0 {
                continue;
            }
            let x = index % 34;
            let y = index / 34;
            assert!((7..27).contains(&x), "{icon:?} x={x}");
            assert!((8..28).contains(&y), "{icon:?} y={y}");
        }
    }

    let (_, branch) = decode_icon(icon_row(SidebarIcon::GitBranch, Some([12, 34, 56])));
    let branch_color = crate::platform::ghostty_image_color([12, 34, 56]);
    assert!(branch
        .chunks_exact(4)
        .any(|pixel| { (1..255).contains(&pixel[3]) && pixel[..3] == branch_color }));
    assert!(branch
        .chunks_exact(4)
        .filter(|pixel| pixel[3] == 255)
        .all(|pixel| pixel[..3] == branch_color));

    let (_, pi) = decode_icon(icon_row(SidebarIcon::Pi, None));
    let ink = pi
        .chunks_exact(4)
        .enumerate()
        .filter(|(_, pixel)| pixel[3] > 0)
        .map(|(index, _)| (index % 34, index / 34))
        .collect::<Vec<_>>();
    assert_eq!(
        (
            ink.iter().map(|(x, _)| *x).min(),
            ink.iter().map(|(x, _)| x + 1).max(),
            ink.iter().map(|(_, y)| *y).min(),
            ink.iter().map(|(_, y)| y + 1).max(),
        ),
        (Some(7), Some(27), Some(8), Some(28)),
    );
    let white = crate::platform::ghostty_image_color([255, 255, 255]);
    assert!(pi
        .chunks_exact(4)
        .filter(|pixel| pixel[3] > 0)
        .all(|pixel| pixel[..3] == white));

    let (_, claude) = decode_icon(icon_row(SidebarIcon::Claude, None));
    let orange = crate::platform::ghostty_image_color([217, 119, 87]);
    assert!(claude
        .chunks_exact(4)
        .filter(|pixel| pixel[3] == 255)
        .any(|pixel| pixel[..3] == orange));

    let (_, codex) = decode_icon(icon_row(SidebarIcon::Codex, None));
    let colors = codex
        .chunks_exact(4)
        .filter(|pixel| pixel[3] == 255)
        .map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect::<HashSet<_>>();
    assert!(colors.len() > 10, "gradient colors: {}", colors.len());
}

#[test]
fn post_styled_final_branch_rgb_colors_the_svg() {
    use crate::ui::SidebarIcon;

    let palette = fixture(0).2;
    let icons = [SidebarIconPlacement {
        icon: SidebarIcon::GitBranch,
        rect: Rect::new(0, 0, 2, 1),
    }];
    let mut buffer = Buffer::empty(Rect::new(0, 0, 2, 1));
    put_text(
        &mut buffer,
        0,
        0,
        2,
        " ",
        Style::default().fg(Color::Rgb(255, 0, 0)),
    );
    let mut frame = FrameData::from_ratatui_buffer(&buffer, None);
    frame.cells[0].fg = crate::protocol::color_to_u32(Color::Rgb(12, 34, 56));
    let mut frames = PaneFrames::default();
    frames.compose(
        &mut frame,
        ChromeLayout {
            panes: &[],
            pane_area: Rect::default(),
            sidebar: None,
            active_tab: None,
            icons: &icons,
        },
        CELL,
        &palette,
        &surface::Occlusion::default(),
    );

    let cached = frames.icon_cache.values().next().expect("branch raster");
    let mut reader = png::Decoder::new(std::io::Cursor::new(&cached.png))
        .read_info()
        .unwrap();
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut pixels).unwrap();
    pixels.truncate(info.buffer_size());
    let expected = crate::platform::ghostty_image_color([12, 34, 56]);
    let opaque = pixels
        .chunks_exact(4)
        .find(|pixel| pixel[3] == 255)
        .expect("opaque branch pixel");
    assert_eq!(opaque[..3], expected);
}

#[test]
fn non_rgb_final_branch_foregrounds_keep_the_powerline_fallback() {
    use crate::ui::SidebarIcon;

    let palette = fixture(0).2;
    let icons = [SidebarIconPlacement {
        icon: SidebarIcon::GitBranch,
        rect: Rect::new(0, 0, 2, 1),
    }];
    for foreground in [Color::Indexed(2), Color::Reset] {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 2, 1));
        put_text(&mut buffer, 0, 0, 2, " ", Style::default().fg(foreground));
        let mut frame = FrameData::from_ratatui_buffer(&buffer, None);
        let bytes = PaneFrames::default().compose(
            &mut frame,
            ChromeLayout {
                panes: &[],
                pane_area: Rect::default(),
                sidebar: None,
                active_tab: None,
                icons: &icons,
            },
            CELL,
            &palette,
            &surface::Occlusion::default(),
        );
        assert!(!bytes.windows(5).any(|bytes| bytes == b"a=t,t"));
        assert_eq!(frame.cells[0].symbol, "");
    }
}

#[test]
fn sidebar_icons_share_rasters_clear_only_placed_branch_fallbacks_and_retire() {
    use crate::ui::SidebarIcon;

    let palette = fixture(0).2;
    let icons = [
        SidebarIconPlacement {
            icon: SidebarIcon::GitBranch,
            rect: Rect::new(1, 1, 2, 1),
        },
        SidebarIconPlacement {
            icon: SidebarIcon::Pi,
            rect: Rect::new(1, 2, 2, 1),
        },
        SidebarIconPlacement {
            icon: SidebarIcon::Pi,
            rect: Rect::new(5, 2, 2, 1),
        },
    ];
    let mut buffer = Buffer::empty(Rect::new(0, 0, 12, 4));
    put_text(
        &mut buffer,
        1,
        1,
        2,
        " ",
        Style::default().fg(Color::Rgb(91, 89, 92)),
    );
    let original = FrameData::from_ratatui_buffer(&buffer, None);
    let chrome = |icons| ChromeLayout {
        panes: &[],
        pane_area: Rect::default(),
        sidebar: None,
        active_tab: None,
        icons,
    };
    let mut frames = PaneFrames::default();
    frames.set_scope("sidebar-icons");
    let mut frame = original.clone();
    let first = frames.compose(
        &mut frame,
        chrome(&icons),
        CELL,
        &palette,
        &surface::Occlusion::default(),
    );
    assert_eq!(
        first.windows(5).filter(|bytes| *bytes == b"a=t,t").count(),
        2
    );
    assert_eq!(frames.icon_rasterizations, 2);
    assert_eq!(frames.icon_cache.len(), 2);
    assert_eq!(frame.cells[13].symbol, " ");
    assert_eq!(frame.cells[14].symbol, " ");

    let mut replay_frame = original.clone();
    let replay = frames.compose(
        &mut replay_frame,
        chrome(&icons),
        CELL,
        &palette,
        &surface::Occlusion::default(),
    );
    assert!(!replay.windows(5).any(|bytes| bytes == b"a=t,t"));
    assert_eq!(frames.icon_rasterizations, 2);
    assert_eq!(replay_frame.cells[13].symbol, " ");

    let mut occlusion = surface::Occlusion::default();
    occlusion.cover(icons[0].rect);
    let mut occluded_frame = original.clone();
    frames.compose(
        &mut occluded_frame,
        chrome(&icons),
        CELL,
        &palette,
        &occlusion,
    );
    assert_eq!(occluded_frame.cells[13].symbol, "");

    let mut changed_frame = original.clone();
    changed_frame.cells[13].fg = crate::protocol::color_to_u32(Color::Rgb(255, 0, 0));
    frames.compose(
        &mut changed_frame,
        chrome(&icons),
        CELL,
        &palette,
        &surface::Occlusion::default(),
    );
    assert_eq!(frames.icon_rasterizations, 3);

    let mut resized_frame = original.clone();
    resized_frame.cells[13].fg = crate::protocol::color_to_u32(Color::Rgb(255, 0, 0));
    frames.compose(
        &mut resized_frame,
        chrome(&icons),
        HostCellSize {
            width_px: 18,
            height_px: 38,
        },
        &palette,
        &surface::Occlusion::default(),
    );
    assert_eq!(frames.icon_rasterizations, 5);

    let cleanup = frames.cleanup();
    assert!(!cleanup.is_empty());
    assert!(frames.rows.is_empty());
}

#[test]
fn sidebar_icon_cache_is_bounded_across_branch_colors() {
    use crate::ui::SidebarIcon;

    let palette = fixture(0).2;
    let icons = (0..40)
        .map(|row| SidebarIconPlacement {
            icon: SidebarIcon::GitBranch,
            rect: Rect::new(0, row, 2, 1),
        })
        .collect::<Vec<_>>();
    let mut buffer = Buffer::empty(Rect::new(0, 0, 2, 40));
    for row in 0..40 {
        buffer[(0, row)]
            .set_symbol("")
            .set_fg(Color::Rgb(row as u8, 20, 30));
    }
    let mut frame = FrameData::from_ratatui_buffer(&buffer, None);
    let mut frames = PaneFrames::default();
    frames.compose(
        &mut frame,
        ChromeLayout {
            panes: &[],
            pane_area: Rect::default(),
            sidebar: None,
            active_tab: None,
            icons: &icons,
        },
        CELL,
        &palette,
        &surface::Occlusion::default(),
    );
    assert_eq!(frames.icon_rasterizations, 40);
    assert_eq!(frames.icon_cache.len(), MAX_ICON_CACHE_ENTRIES);
}

#[test]
fn invalid_or_occluded_tab_underlines_keep_the_font_fallback() {
    let palette = fixture(0).2;
    assert_font_underline_fallback(
        Some(Rect::new(2, 0, 8, 1)),
        HostCellSize {
            width_px: 1,
            height_px: 1,
        },
        &palette,
        &surface::Occlusion::default(),
    );
    assert_font_underline_fallback(
        Some(Rect::new(18, 0, 8, 1)),
        CELL,
        &palette,
        &surface::Occlusion::default(),
    );
    let mut non_rgb = palette.clone();
    non_rgb.accent = Color::Indexed(2);
    assert_font_underline_fallback(
        Some(Rect::new(2, 0, 8, 1)),
        CELL,
        &non_rgb,
        &surface::Occlusion::default(),
    );
    let mut occlusion = surface::Occlusion::default();
    occlusion.cover(Rect::new(2, 0, 8, 1));
    assert_font_underline_fallback(Some(Rect::new(2, 0, 8, 1)), CELL, &palette, &occlusion);
}

#[test]
fn pixel_tab_underline_preserves_labels_colors_and_other_modifiers() {
    let palette = fixture(0).2;
    let mut frames = PaneFrames::default();
    frames.set_scope("tabs");
    let mut frame = tab_frame();
    let original = frame.cells[2..10].to_vec();
    let bytes = frames.compose(
        &mut frame,
        layout(&[], Some(Rect::new(2, 0, 8, 1))),
        CELL,
        &palette,
        &surface::Occlusion::default(),
    );
    assert!(bytes.windows(5).any(|bytes| bytes == b"a=t,t"));
    for (before, after) in original.iter().zip(&frame.cells[2..10]) {
        assert_eq!(after.symbol, before.symbol);
        assert_eq!(after.fg, before.fg);
        assert_eq!(after.bg, before.bg);
        assert_eq!(
            after.modifier & Modifier::BOLD.bits(),
            Modifier::BOLD.bits()
        );
        assert_eq!(after.modifier & Modifier::UNDERLINED.bits(), 0);
    }
}

#[test]
fn tab_underline_fallback_and_focus_changes_retain_the_font_indicator() {
    let palette = fixture(0).2;
    let mut frames = PaneFrames::default();
    frames.set_scope("tabs");
    let mut first = tab_frame();
    let initial = frames.compose(
        &mut first,
        layout(&[], Some(Rect::new(2, 0, 8, 1))),
        CELL,
        &palette,
        &surface::Occlusion::default(),
    );
    assert!(initial.windows(5).any(|bytes| bytes == b"a=t,t"));
    let mut redraw = tab_frame();
    let replay = frames.compose(
        &mut redraw,
        layout(&[], Some(Rect::new(2, 0, 8, 1))),
        CELL,
        &palette,
        &surface::Occlusion::default(),
    );
    assert!(!replay.windows(5).any(|bytes| bytes == b"a=t,t"));

    let mut focused = tab_frame();
    let changed = frames.compose(
        &mut focused,
        layout(&[], Some(Rect::new(11, 0, 7, 1))),
        CELL,
        &palette,
        &surface::Occlusion::default(),
    );
    assert!(changed.windows(5).any(|bytes| bytes == b"a=t,t"));
    assert!(String::from_utf8_lossy(&changed).contains("a=d,d=I"));
    assert!(focused.cells[2..10]
        .iter()
        .all(|cell| cell.modifier & Modifier::UNDERLINED.bits() != 0));
}

#[test]
fn pixel_frames_preserve_labels_and_reuse_images_during_redraws() {
    let (original, panes, palette) = fixture(1);
    let mut frames = PaneFrames::default();
    frames.set_scope("test");
    let mut frame = original.clone();
    let bytes = frames.compose(
        &mut frame,
        layout(&panes, None),
        CELL,
        &palette,
        &surface::Occlusion::default(),
    );
    assert!(bytes.windows(5).any(|bytes| bytes == b"a=t,t"));
    assert_eq!(frame.cells[0].symbol, " ");
    assert_eq!(
        frame.cells[1..8]
            .iter()
            .map(|cell| cell.symbol.as_str())
            .collect::<String>(),
        " label "
    );
    assert!(frames.take_pending_cleanup().is_empty());
    let bytes = frames.compose(
        &mut original.clone(),
        layout(&panes, None),
        CELL,
        &palette,
        &surface::Occlusion::default(),
    );
    assert!(!bytes.windows(5).any(|bytes| bytes == b"a=t,t"));
    assert!(
        !bytes.is_empty(),
        "replay placements without uploading pixels"
    );
    frames.set_scope("next-workspace");
    assert!(
        !frames.take_pending_cleanup().is_empty(),
        "retire the previous workspace's images"
    );
}

#[test]
fn overlays_keep_their_text_and_occlude_border_images() {
    let (mut frame, panes, palette) = fixture(1);
    let mut frames = PaneFrames::default();
    frames.set_scope("test");
    let mut occlusion = surface::Occlusion::default();
    occlusion.cover(Rect::new(0, 0, 120, 1));
    frames.compose(&mut frame, layout(&panes, None), CELL, &palette, &occlusion);
    assert_eq!(
        frame.cells[0].symbol, "🭽",
        "occluded rows retain their text fallback"
    );
    assert_eq!(
        frame.cells[120 * 39].symbol,
        " ",
        "unoccluded bottom uses pixels"
    );
    let cleanup = frames.cleanup();
    assert!(
        !cleanup.is_empty(),
        "hidden frames must remove their images"
    );
    assert!(frames.cleanup().is_empty());
}

fn border_pixels(row: &BorderRow) -> (usize, Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(row.png().unwrap()))
        .read_info()
        .unwrap();
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut pixels).unwrap();
    (info.width as usize, pixels)
}

#[test]
fn sidebar_rows_use_sidebar_interior_and_remain_independent_of_pane_colors() {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 12, 6));
    let mut palette = fixture(0).2;
    palette.sidebar_bg = Color::Rgb(45, 40, 55);
    palette.pane_default_bg = Color::Reset;
    buffer.set_style(buffer.area, Style::default().bg(palette.pane_gap_bg));
    super::super::render::render_sidebar_frame(&mut buffer, Rect::new(1, 0, 5, 6), &palette);
    let mut frame = FrameData::from_ratatui_buffer(&buffer, None);
    let sidebar_background = crate::protocol::color_to_u32(palette.sidebar_bg);
    for (x, y) in [(1, 0), (1, 1), (3, 3), (5, 4), (5, 5)] {
        assert_eq!(frame.cells[y * 12 + x].bg, sidebar_background);
    }
    assert_eq!(frame.cells[1].symbol, "🭽");
    assert_eq!(frame.cells[5].symbol, "🭾");
    let mut frames = PaneFrames::default();
    frames.set_scope("sidebar");
    let bytes = frames.compose(
        &mut frame,
        ChromeLayout {
            panes: &[],
            pane_area: Rect::default(),
            sidebar: Some(Rect::new(1, 0, 5, 6)),
            active_tab: None,
            icons: &[],
        },
        CELL,
        &palette,
        &surface::Occlusion::default(),
    );

    assert_eq!(
        bytes.windows(5).filter(|bytes| *bytes == b"a=t,t").count(),
        2
    );
    assert_eq!(frames.rows.len(), 2);
    for row in &frames.rows {
        let ChromeRow::Border(row) = row else {
            panic!("sidebar chrome row must be a border");
        };
        assert_eq!(row.inside, [45, 40, 55]);
        assert_eq!(row.outside, [34, 31, 34]);
        assert_eq!(row.stroke, [91, 89, 92]);
        assert_eq!(row.title_span, None);
    }
    assert_eq!(frame.cells[1].symbol, " ");
    assert_eq!(frame.cells[5].symbol, " ");
}

#[test]
fn title_chip_fills_the_padded_span_without_covering_adjacent_border() {
    let (green, gray, inside, outside) = if cfg!(target_os = "macos") {
        ([179, 219, 130], [91, 89, 92], [30, 30, 45], [33, 31, 34])
    } else {
        ([169, 220, 118], [91, 89, 92], [30, 30, 46], [34, 31, 34])
    };
    for (stroke, expected_stroke) in [([169, 220, 118], green), ([91, 89, 92], gray)] {
        let row = BorderRow {
            rect: Rect::new(0, 0, 12, 1),
            edge: Edge::Top,
            cell_width: 17,
            cell_height: 36,
            inside: [30, 30, 46],
            outside: [34, 31, 34],
            stroke,
            title_span: Some((1, 8)),
        };
        let (width, pixels) = border_pixels(&row);
        let pixel = |x: usize, y: usize| &pixels[(y * width + x) * 3..][..3];

        for x in 17..8 * 17 {
            assert_eq!(pixel(x, 4), expected_stroke);
            assert_eq!(pixel(x, 31), expected_stroke);
        }
        assert_eq!(pixel(8 * 17, 4), outside);
        assert_eq!(pixel(8 * 17, 17), expected_stroke);
        assert_eq!(pixel(8 * 17, 31), inside);
    }
}

#[test]
fn rendered_wide_title_drives_chip_range_and_preserves_title_glyphs() {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 12, 3));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(crate::ui::PANE_BORDER_SET)
        .border_style(Style::default().fg(Color::Rgb(169, 220, 118)))
        .title_style(
            Style::default()
                .fg(Color::Rgb(0, 0, 0))
                .bg(Color::Rgb(169, 220, 118)),
        )
        .title(" 模块🭽界 ");
    let inner = block.inner(buffer.area);
    block.render(buffer.area, &mut buffer);
    let mut frame = FrameData::from_ratatui_buffer(&buffer, None);
    let panes = [PaneSurfacePane {
        pane_id: "wide-title".into(),
        content_revision: 0,
        rect: buffer.area.into(),
        inner_rect: inner.into(),
        scrollbar_rect: None,
        scroll: None,
        focused: true,
        mouse_reporting: false,
        sgr_pixel_mouse: false,
        alternate_screen_active: false,
        pixel_width: 0,
        pixel_height: 0,
    }];
    let mut palette = fixture(0).2;
    palette.accent = Color::Rgb(255, 0, 255);
    let mut frames = PaneFrames::default();
    frames.set_scope("styled-title");

    frames.compose(
        &mut frame,
        layout(&panes, None),
        CELL,
        &palette,
        &surface::Occlusion::default(),
    );

    let ChromeRow::Border(top) = &frames.rows[0] else {
        panic!("first row should be the top border");
    };
    assert_eq!(top.stroke, [169, 220, 118]);
    assert_eq!(top.title_span, Some((1, 10)));
    assert_eq!(
        frame.cells[1..11]
            .iter()
            .map(|cell| cell.symbol.as_str())
            .collect::<Vec<_>>(),
        [" ", "模", " ", "块", " ", "🭽", "界", " ", " ", " "],
    );
}

#[test]
fn border_colored_pane_background_does_not_create_a_title_span() {
    let (mut frame, panes, mut palette) = fixture(1);
    palette.pane_default_bg = palette.accent;
    let stroke = crate::protocol::color_to_u32(palette.accent);
    for cell in &mut frame.cells {
        cell.bg = stroke;
    }
    for cell in &mut frame.cells[1..8] {
        cell.symbol = "▔".into();
        cell.fg = stroke;
    }
    let mut frames = PaneFrames::default();
    frames.set_scope("no-false-title");

    frames.compose(
        &mut frame,
        layout(&panes, None),
        CELL,
        &palette,
        &surface::Occlusion::default(),
    );

    let ChromeRow::Border(top) = &frames.rows[0] else {
        panic!("first row should be the top border");
    };
    assert_eq!(top.title_span, None);
}

#[test]
fn focus_rename_and_title_removal_refresh_cached_border_images() {
    let (base_frame, mut panes, palette) = fixture(1);
    let mut frames = PaneFrames::default();
    frames.set_scope("title-cache");
    let compose = |frames: &mut PaneFrames, frame: &mut FrameData, panes: &[PaneSurfacePane]| {
        frames.compose(
            frame,
            layout(panes, None),
            CELL,
            &palette,
            &surface::Occlusion::default(),
        )
    };
    let mut frame = base_frame.clone();
    assert!(compose(&mut frames, &mut frame, &panes)
        .windows(5)
        .any(|bytes| bytes == b"a=t,t"));
    let mut frame = base_frame.clone();
    assert!(!compose(&mut frames, &mut frame, &panes)
        .windows(5)
        .any(|bytes| bytes == b"a=t,t"));

    let accent = crate::protocol::color_to_u32(palette.accent);
    let overlay = crate::protocol::color_to_u32(palette.overlay0);
    let pane_bg = crate::protocol::color_to_u32(palette.pane_default_bg);
    let white = crate::protocol::color_to_u32(Color::Rgb(255, 255, 255));
    let mut focused_frame = base_frame.clone();
    for cell in &mut focused_frame.cells {
        if cell.fg == accent {
            cell.fg = overlay;
        }
        if cell.bg == accent {
            cell.bg = overlay;
            cell.fg = white;
        }
    }
    panes[0].focused = false;
    let mut frame = focused_frame.clone();
    assert!(compose(&mut frames, &mut frame, &panes)
        .windows(5)
        .any(|bytes| bytes == b"a=t,t"));

    let mut renamed_frame = focused_frame.clone();
    for cell in &mut renamed_frame.cells[6..8] {
        cell.symbol = "▔".into();
        cell.fg = overlay;
        cell.bg = pane_bg;
    }
    let mut frame = renamed_frame.clone();
    assert!(compose(&mut frames, &mut frame, &panes)
        .windows(5)
        .any(|bytes| bytes == b"a=t,t"));

    for cell in &mut renamed_frame.cells[1..6] {
        cell.symbol = "▔".into();
        cell.fg = overlay;
        cell.bg = pane_bg;
    }
    assert!(compose(&mut frames, &mut renamed_frame, &panes)
        .windows(5)
        .any(|bytes| bytes == b"a=t,t"));
}

#[test]
fn tall_narrow_border_geometry_stays_bounded() {
    let (stroke, outside) = if cfg!(target_os = "macos") {
        ([179, 219, 130], [33, 31, 34])
    } else {
        ([169, 220, 118], [34, 31, 34])
    };
    for (cell_width, cell_height, top_line, bottom_line) in [(2, 20, 2, 19), (17, 12, 7, 0)] {
        let mut exterior_rows = [0; 2];
        for (edge_index, edge) in [Edge::Top, Edge::Bottom].into_iter().enumerate() {
            let row = BorderRow {
                rect: Rect::new(0, 0, 24, 1),
                edge,
                cell_width,
                cell_height,
                inside: [30, 30, 46],
                outside: [34, 31, 34],
                stroke: [169, 220, 118],
                title_span: None,
            };
            let (width, pixels) = border_pixels(&row);
            let pixel = |x: usize, y: usize| &pixels[(y * width + x) * 3..][..3];
            let x = width / 2;
            let line = if edge == Edge::Top {
                top_line
            } else {
                bottom_line
            };
            assert_eq!(pixel(x, line), stroke);
            if edge == Edge::Top && top_line > 0 {
                assert_eq!(pixel(x, 0), outside);
            }
            if edge == Edge::Bottom && bottom_line + 1 < cell_height as usize {
                assert_eq!(pixel(x, cell_height as usize - 1), outside);
            }
            exterior_rows[edge_index] = (0..cell_height as usize)
                .filter(|y| pixel(x, *y) == outside)
                .count();
        }
        assert_eq!(exterior_rows.iter().sum::<usize>(), cell_width as usize);
        assert_eq!(exterior_rows, [top_line, cell_width as usize - top_line]);
    }
}

#[test]
fn rounded_border_strips_cut_out_corners_and_join_straight_edges() {
    for (edge, outer_y, side_y, blended_y) in [(Edge::Top, 17, 29, 25), (Edge::Bottom, 35, 23, 27)]
    {
        let row = BorderRow {
            rect: Rect::new(0, 0, 24, 1),
            edge,
            cell_width: 17,
            cell_height: 36,
            inside: [0; 3],
            outside: [32; 3],
            stroke: [255; 3],
            title_span: None,
        };
        let mut reader = png::Decoder::new(std::io::Cursor::new(row.png().unwrap()))
            .read_info()
            .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut pixels).unwrap();
        let width = info.width as usize;
        let pixel = |x: usize, y: usize| &pixels[(y * width + x) * 3..][..3];
        for x in [0, width - 1] {
            assert_eq!(
                pixel(x, outer_y),
                [32; 3],
                "corner must expose the exterior"
            );
            assert_eq!(
                pixel(x, side_y),
                [255; 3],
                "arc must join the vertical edge"
            );
            assert_eq!(pixel(x, blended_y), [139; 3], "curve must be antialiased");
        }
        assert_eq!(
            pixel(12, outer_y),
            [255; 3],
            "arc must join the horizontal edge at 12 pixels"
        );
        assert_eq!(pixel(12, side_y), [0; 3], "pane interior remains unchanged");
    }
}

fn cached_profile_fixture(
    count: usize,
) -> (
    FrameData,
    Vec<PaneSurfacePane>,
    Palette,
    Rect,
    Rect,
    Rect,
    Vec<SidebarIconPlacement>,
) {
    use crate::ui::SidebarIcon;

    let (pane_frame, panes, palette) = fixture(count);
    let sidebar = Rect::new(1, 0, 20, 40);
    let pane_area = Rect::new(22, 0, 120, 40);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 142, 40));
    buffer.set_style(buffer.area, Style::default().bg(palette.pane_gap_bg));
    super::super::render::render_sidebar_frame(&mut buffer, sidebar, &palette);
    for (y, text, style) in [
        (4, " main", Style::default().fg(palette.overlay0)),
        (5, "  Pi", Style::default().fg(palette.text)),
        (6, "  Claude", Style::default().fg(palette.text)),
        (7, "  Codex", Style::default().fg(palette.text)),
    ] {
        put_text(&mut buffer, 3, y, 16, text, style);
    }
    let icons = [
        SidebarIcon::GitBranch,
        SidebarIcon::Pi,
        SidebarIcon::Claude,
        SidebarIcon::Codex,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, icon)| SidebarIconPlacement {
        icon,
        rect: Rect::new(3, 4 + index as u16, 2, 1),
    })
    .collect();
    let mut frame = FrameData::from_ratatui_buffer(&buffer, None);
    for y in 0..40_usize {
        for x in 0..120_usize {
            frame.cells[y * 142 + 22 + x] = pane_frame.cells[y * 120 + x].clone();
        }
    }
    (
        frame,
        panes,
        palette,
        sidebar,
        pane_area,
        Rect::new(30, 0, 8, 1),
        icons,
    )
}

#[test]
#[ignore]
fn pixel_pane_frame_render_scale_profile() {
    for count in [1, 15] {
        let (original, panes, palette, sidebar, pane_area, active_tab, icons) =
            cached_profile_fixture(count);
        let mut frames = PaneFrames::default();
        frames.set_scope("benchmark");
        let occlusion = surface::Occlusion::default();
        let chrome = || ChromeLayout {
            panes: &panes,
            pane_area,
            sidebar: Some(sidebar),
            active_tab: Some(active_tab),
            icons: &icons,
        };
        frames.compose(&mut original.clone(), chrome(), CELL, &palette, &occlusion);
        assert_eq!(frames.icon_rasterizations, 4);
        let mut samples = Vec::new();
        for _ in 0..101 {
            let mut frame = original.clone();
            let start = std::time::Instant::now();
            let bytes = frames.compose(&mut frame, chrome(), CELL, &palette, &occlusion);
            samples.push(start.elapsed().as_micros());
            assert!(!bytes.windows(5).any(|bytes| bytes == b"a=t,t"));
            assert_eq!(frames.icon_rasterizations, 4);
        }
        samples.sort_unstable();
        eprintln!(
            "cached pixel borders, sidebar, tab underline, and 4 SVG icons: {count} panes, median {} us/frame; {} warm rasterizations",
            samples[50], frames.icon_rasterizations
        );
    }
}
