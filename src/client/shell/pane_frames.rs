use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io;

use super::*;
use crate::kitty_graphics::{surface, HostCellSize};
#[cfg(test)]
#[path = "tests/pane_frames.rs"]
mod tests;

use crate::protocol::{
    PaneSurfacePane, SurfaceGraphicsAsset, SurfaceGraphicsAssetKey, SurfaceGraphicsFormat,
    SurfaceGraphicsPlacement, SurfaceGraphicsScene, SurfaceGraphicsSource,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Edge {
    Top,
    Bottom,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct BorderRow {
    rect: Rect,
    edge: Edge,
    cell_width: u32,
    cell_height: u32,
    inside: [u8; 3],
    outside: [u8; 3],
    stroke: [u8; 3],
    title_span: Option<(u16, u16)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TabUnderline {
    rect: Rect,
    cell_width: u32,
    cell_height: u32,
    accent: [u8; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct SidebarIconPlacement {
    pub(super) icon: crate::ui::SidebarIcon,
    pub(super) rect: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum SidebarDecoration {
    Icon(SidebarIconPlacement),
    Highlight(Rect),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct IconRow {
    placement: SidebarIconPlacement,
    cell_width: u32,
    cell_height: u32,
    color: Option<[u8; 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RoundedHighlight {
    rect: Rect,
    cell_width: u32,
    cell_height: u32,
    fill: [u8; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct IconAssetKey {
    icon: crate::ui::SidebarIcon,
    cell_width: u32,
    cell_height: u32,
    color: Option<[u8; 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct HighlightAssetKey {
    width: u16,
    height: u16,
    cell_width: u32,
    cell_height: u32,
    fill: [u8; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum SidebarAssetKey {
    Icon(IconAssetKey),
    Highlight(HighlightAssetKey),
}

#[derive(Debug)]
struct CachedSidebarAsset {
    png: Vec<u8>,
    last_used: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ChromeRow {
    Border(BorderRow),
    TabUnderline(TabUnderline),
    Icon(IconRow),
    RoundedHighlight(RoundedHighlight),
}

pub(super) struct ChromeLayout<'a> {
    pub(super) panes: &'a [PaneSurfacePane],
    pub(super) pane_area: Rect,
    pub(super) sidebar: Option<Rect>,
    pub(super) active_tab: Option<Rect>,
    pub(super) decorations: &'a [SidebarDecoration],
    pub(super) host_theme: &'a crate::terminal_theme::TerminalTheme,
}

const CORNER_RADIUS_PX: u32 = 12;
const HIGHLIGHT_RADIUS_PX: u32 = 6;
const HIGHLIGHT_VERTICAL_INSET_PX: u32 = 2;
const MAX_SIDEBAR_ASSET_CACHE_ENTRIES: usize = 32;

fn host_rgb(
    color: Color,
    theme: &crate::terminal_theme::TerminalTheme,
    default: Option<crate::terminal_theme::RgbColor>,
) -> Option<[u8; 3]> {
    let color = match color {
        Color::Rgb(red, green, blue) => return Some([red, green, blue]),
        Color::Indexed(index) => theme.palette[usize::from(index)]?,
        Color::Reset => default?,
        Color::Black => theme.palette[0]?,
        Color::Red => theme.palette[1]?,
        Color::Green => theme.palette[2]?,
        Color::Yellow => theme.palette[3]?,
        Color::Blue => theme.palette[4]?,
        Color::Magenta => theme.palette[5]?,
        Color::Cyan => theme.palette[6]?,
        Color::Gray => theme.palette[7]?,
        Color::DarkGray => theme.palette[8]?,
        Color::LightRed => theme.palette[9]?,
        Color::LightGreen => theme.palette[10]?,
        Color::LightYellow => theme.palette[11]?,
        Color::LightBlue => theme.palette[12]?,
        Color::LightMagenta => theme.palette[13]?,
        Color::LightCyan => theme.palette[14]?,
        Color::White => theme.palette[15]?,
    };
    Some([color.r, color.g, color.b])
}

pub(super) fn pixel_chrome_available(enabled: bool, cell: HostCellSize) -> bool {
    enabled
        && cell.is_known()
        && cell.height_px > cell.width_px.div_ceil(2) + (cell.width_px / 8).max(1)
}

pub(super) fn record_token_icons(
    icons: &[crate::ui::TokenIcon],
    origin: (u16, u16),
    right: u16,
    decorations: &mut Vec<SidebarDecoration>,
) {
    decorations.extend(icons.iter().filter_map(|icon| {
        let column = u16::try_from(icon.column).ok()?;
        let x = origin.0.checked_add(column)?;
        let rect = Rect::new(x, origin.1, 2, 1);
        (rect.right() <= right).then_some(SidebarDecoration::Icon(SidebarIconPlacement {
            icon: icon.icon,
            rect,
        }))
    }));
}

impl BorderRow {
    fn png(&self) -> std::io::Result<Vec<u8>> {
        let width = u32::from(self.rect.width)
            .checked_mul(self.cell_width)
            .ok_or_else(|| std::io::Error::other("pane border width overflow"))?;
        let height = self.cell_height;
        let len = width.checked_mul(height).and_then(|n| n.checked_mul(3));
        let Some(len) = len.filter(|n| *n <= 4 * 1024 * 1024) else {
            return Err(std::io::Error::other("pane border image exceeds 4 MiB"));
        };
        let thickness = (self.cell_width / 8).max(1);
        let available = height - thickness;
        let top_margin = ((height - thickness) / 2).clamp(
            self.cell_width.saturating_sub(available),
            self.cell_width.min(available),
        );
        let bottom_margin = self.cell_width - top_margin;
        let (line, exterior_margin) = match self.edge {
            Edge::Top => (top_margin, top_margin),
            Edge::Bottom => (
                height.saturating_sub(bottom_margin + thickness),
                bottom_margin,
            ),
        };
        let radius = f64::from(
            CORNER_RADIUS_PX
                .min(width / 2)
                .min(height.saturating_sub(exterior_margin)),
        );
        let title_inset = (self.cell_width / 4).max(1).min(height / 2);
        let title_rows = title_inset..height.saturating_sub(title_inset);
        let inside = crate::platform::ghostty_image_color(self.inside);
        let exterior = crate::platform::ghostty_image_color(self.outside);
        let stroke = crate::platform::ghostty_image_color(self.stroke);
        let mut pixels = Vec::with_capacity(len as usize);
        for y in 0..height {
            let outside = match self.edge {
                Edge::Top => y < line,
                Edge::Bottom => y >= line + thickness,
            };
            let depth = match self.edge {
                Edge::Top => f64::from(y) + 0.5 - f64::from(line),
                Edge::Bottom => f64::from(line + thickness) - f64::from(y) - 0.5,
            };
            for x in 0..width {
                let inset = f64::from(x.min(width - 1 - x)) + 0.5;
                let title_column = self.title_span.is_some_and(|(start, end)| {
                    x >= u32::from(start) * self.cell_width && x < u32::from(end) * self.cell_width
                });
                let color = if title_column && title_rows.contains(&y) {
                    stroke
                } else if outside {
                    exterior
                } else if inset < radius && depth < radius {
                    let distance = radius - (radius - inset).hypot(radius - depth);
                    let outer = (distance + 0.5).clamp(0.0, 1.0);
                    let inner = (distance - f64::from(thickness) + 0.5).clamp(0.0, 1.0);
                    std::array::from_fn(|channel| {
                        (f64::from(exterior[channel]) * (1.0 - outer)
                            + f64::from(stroke[channel]) * (outer - inner)
                            + f64::from(inside[channel]) * inner)
                            .round() as u8
                    })
                } else if (line..line + thickness).contains(&y)
                    || x < thickness
                    || x >= width.saturating_sub(thickness)
                {
                    stroke
                } else {
                    inside
                };
                pixels.extend_from_slice(&color);
            }
        }
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
            writer
                .write_image_data(&pixels)
                .map_err(std::io::Error::other)?;
        }
        Ok(bytes)
    }
}

impl TabUnderline {
    fn png(&self) -> std::io::Result<Vec<u8>> {
        let width = u32::from(self.rect.width)
            .checked_mul(self.cell_width)
            .ok_or_else(|| std::io::Error::other("tab underline width overflow"))?;
        let height = self.cell_height;
        let len = width.checked_mul(height).and_then(|n| n.checked_mul(4));
        let Some(len) = len.filter(|n| *n <= 4 * 1024 * 1024) else {
            return Err(std::io::Error::other("tab underline image exceeds 4 MiB"));
        };
        let thickness = (self.cell_width / 8).max(1).min(height);
        let accent = crate::platform::ghostty_image_color(self.accent);
        let mut pixels = vec![0; len as usize];
        for pixel in
            pixels[(height - thickness) as usize * width as usize * 4..].chunks_exact_mut(4)
        {
            pixel.copy_from_slice(&[accent[0], accent[1], accent[2], 255]);
        }
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
            writer
                .write_image_data(&pixels)
                .map_err(std::io::Error::other)?;
        }
        Ok(bytes)
    }
}

impl IconRow {
    fn asset_key(self) -> SidebarAssetKey {
        SidebarAssetKey::Icon(IconAssetKey {
            icon: self.placement.icon,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
            color: self.color,
        })
    }

    fn png(self) -> io::Result<Vec<u8>> {
        let width = self
            .cell_width
            .checked_mul(2)
            .filter(|width| *width > 0)
            .ok_or_else(|| io::Error::other("invalid sidebar icon width"))?;
        let height = self.cell_height;
        let len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4));
        if len.is_none_or(|len| len > 4 * 1024 * 1024) {
            return Err(io::Error::other("sidebar icon image exceeds 4 MiB"));
        }
        let provider_viewport = (height.saturating_mul(5).saturating_add(4) / 9).min(width);
        let viewport = self
            .placement
            .icon
            .pixel_viewport(provider_viewport, width, height);
        if viewport == 0 {
            return Err(io::Error::other("invalid sidebar icon viewport"));
        }
        let mut options = resvg::usvg::Options::default();
        if let Some([red, green, blue]) = self.color {
            options.style_sheet = Some(format!(
                "svg {{ color: rgb({red}, {green}, {blue}) !important; }}"
            ));
        }
        let svg = match self.placement.icon {
            crate::ui::SidebarIcon::GitBranch => {
                include_bytes!("../../../assets/icons/git-branch.svg").as_slice()
            }
            crate::ui::SidebarIcon::Pi => include_bytes!("../../../assets/icons/pi.svg").as_slice(),
            crate::ui::SidebarIcon::Claude => {
                include_bytes!("../../../assets/icons/claude.svg").as_slice()
            }
            crate::ui::SidebarIcon::Codex => {
                include_bytes!("../../../assets/icons/codex.svg").as_slice()
            }
        };
        let tree = resvg::usvg::Tree::from_data(svg, &options)
            .map_err(|error| io::Error::other(format!("invalid sidebar SVG: {error}")))?;
        let tree_size = tree.size();
        let scale = viewport as f32 / tree_size.width().max(tree_size.height());
        let rendered_width = tree_size.width() * scale;
        let rendered_height = tree_size.height() * scale;
        let x = (width as f32 - rendered_width) / 2.0;
        let y = (height as f32 - rendered_height) / 2.0;
        let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
            .ok_or_else(|| io::Error::other("sidebar icon pixmap is too large"))?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, x, y),
            &mut pixmap.as_mut(),
        );
        for pixel in pixmap.data_mut().chunks_exact_mut(4) {
            let alpha = pixel[3];
            if alpha == 0 {
                pixel[0] = 0;
                pixel[1] = 0;
                pixel[2] = 0;
                continue;
            }
            let demultiply = |channel: u8| {
                ((u32::from(channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha)).min(255)
                    as u8
            };
            let color = crate::platform::ghostty_image_color([
                demultiply(pixel[0]),
                demultiply(pixel[1]),
                demultiply(pixel[2]),
            ]);
            pixel[0] = color[0];
            pixel[1] = color[1];
            pixel[2] = color[2];
        }
        encode_rgba_png(width, height, pixmap.data())
    }
}

impl RoundedHighlight {
    fn asset_key(self) -> SidebarAssetKey {
        SidebarAssetKey::Highlight(HighlightAssetKey {
            width: self.rect.width,
            height: self.rect.height,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
            fill: self.fill,
        })
    }

    fn png(self) -> io::Result<Vec<u8>> {
        let width = u32::from(self.rect.width)
            .checked_mul(self.cell_width)
            .filter(|width| *width > 0)
            .ok_or_else(|| io::Error::other("invalid sidebar highlight width"))?;
        let height = u32::from(self.rect.height)
            .checked_mul(self.cell_height)
            .filter(|height| *height > 0)
            .ok_or_else(|| io::Error::other("invalid sidebar highlight height"))?;
        let len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4));
        let Some(len) = len.filter(|len| *len <= 4 * 1024 * 1024) else {
            return Err(io::Error::other("sidebar highlight image exceeds 4 MiB"));
        };
        let inner_height = height.saturating_sub(HIGHLIGHT_VERTICAL_INSET_PX * 2);
        let radius = f64::from(HIGHLIGHT_RADIUS_PX)
            .min(f64::from(width) / 2.0)
            .min(f64::from(inner_height) / 2.0);
        let fill = crate::platform::ghostty_image_color(self.fill);
        let mut pixels = vec![0; len as usize];
        if inner_height > 0 {
            for y in HIGHLIGHT_VERTICAL_INSET_PX..height - HIGHLIGHT_VERTICAL_INSET_PX {
                let inset_y = f64::from(
                    (y - HIGHLIGHT_VERTICAL_INSET_PX)
                        .min(height - HIGHLIGHT_VERTICAL_INSET_PX - 1 - y),
                ) + 0.5;
                for x in 0..width {
                    let inset_x = f64::from(x.min(width - 1 - x)) + 0.5;
                    let coverage = if inset_x < radius && inset_y < radius {
                        let distance = radius - (radius - inset_x).hypot(radius - inset_y);
                        (distance + 0.5).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    let alpha = (coverage * 255.0).round() as u8;
                    if alpha == 0 {
                        continue;
                    }
                    let offset = ((y * width + x) * 4) as usize;
                    pixels[offset..offset + 4].copy_from_slice(&[fill[0], fill[1], fill[2], alpha]);
                }
            }
        }
        encode_rgba_png(width, height, &pixels)
    }
}

impl SidebarAssetKey {
    fn layer_id(self) -> String {
        match self {
            Self::Icon(key) => {
                let kind = match key.icon {
                    crate::ui::SidebarIcon::GitBranch => "git-branch",
                    crate::ui::SidebarIcon::Pi => "pi",
                    crate::ui::SidebarIcon::Claude => "claude",
                    crate::ui::SidebarIcon::Codex => "codex",
                };
                let color = key
                    .color
                    .map(|[red, green, blue]| format!("-{red:02x}{green:02x}{blue:02x}"))
                    .unwrap_or_default();
                format!(
                    "sidebar-{kind}-{}x{}{color}",
                    key.cell_width, key.cell_height
                )
            }
            Self::Highlight(key) => {
                let [red, green, blue] = key.fill;
                format!(
                    "sidebar-highlight-{}x{}-{}x{}-{red:02x}{green:02x}{blue:02x}",
                    key.width, key.height, key.cell_width, key.cell_height
                )
            }
        }
    }
}

fn encode_rgba_png(width: u32, height: u32, pixels: &[u8]) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(io::Error::other)?;
        writer.write_image_data(pixels).map_err(io::Error::other)?;
    }
    Ok(bytes)
}

impl ChromeRow {
    fn rect(&self) -> Rect {
        match self {
            Self::Border(row) => row.rect,
            Self::TabUnderline(row) => row.rect,
            Self::Icon(row) => row.placement.rect,
            Self::RoundedHighlight(row) => row.rect,
        }
    }

    fn png(&self) -> io::Result<Vec<u8>> {
        match self {
            Self::Border(row) => row.png(),
            Self::TabUnderline(row) => row.png(),
            Self::Icon(row) => row.png(),
            Self::RoundedHighlight(row) => row.png(),
        }
    }

    fn layer_id(&self, index: usize) -> String {
        match self {
            Self::Border(_) => format!("border-{index}"),
            Self::TabUnderline(_) => format!("tab-underline-{index}"),
            Self::Icon(row) => row.asset_key().layer_id(),
            Self::RoundedHighlight(row) => row.asset_key().layer_id(),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct PaneFrames {
    rows: Vec<ChromeRow>,
    graphics: surface::ClientState,
    replay: Vec<u8>,
    sidebar_asset_cache: HashMap<SidebarAssetKey, CachedSidebarAsset>,
    sidebar_asset_cache_clock: u64,
    #[cfg(test)]
    sidebar_asset_rasterizations: usize,
}

impl PaneFrames {
    pub(super) fn set_scope(&mut self, scope: &str) {
        let scope = format!("pane-frames/{scope}");
        if self.graphics.scope() != scope {
            self.rows.clear();
            self.replay.clear();
            self.graphics.set_scope(&scope);
        }
    }

    pub(super) fn take_pending_cleanup(&mut self) -> Vec<u8> {
        self.graphics.take_pending_cleanup()
    }

    pub(super) fn cleanup(&mut self) -> Vec<u8> {
        self.rows.clear();
        self.replay.clear();
        self.graphics.set_scene(SurfaceGraphicsScene::default());
        self.graphics.encode(
            surface::Visibility::Hidden,
            (0, 0),
            None,
            HostCellSize {
                width_px: 1,
                height_px: 1,
            },
            &surface::Occlusion::default(),
        )
    }

    fn sidebar_asset_png(&mut self, row: &ChromeRow) -> io::Result<Vec<u8>> {
        self.sidebar_asset_cache_clock = self.sidebar_asset_cache_clock.wrapping_add(1);
        let last_used = self.sidebar_asset_cache_clock;
        let key = match row {
            ChromeRow::Icon(row) => row.asset_key(),
            ChromeRow::RoundedHighlight(row) => row.asset_key(),
            ChromeRow::Border(_) | ChromeRow::TabUnderline(_) => {
                return row.png();
            }
        };
        if let Some(cached) = self.sidebar_asset_cache.get_mut(&key) {
            cached.last_used = last_used;
            return Ok(cached.png.clone());
        }
        let png = row.png()?;
        #[cfg(test)]
        {
            self.sidebar_asset_rasterizations += 1;
        }
        self.sidebar_asset_cache.insert(
            key,
            CachedSidebarAsset {
                png: png.clone(),
                last_used,
            },
        );
        Ok(png)
    }

    fn trim_sidebar_asset_cache(&mut self, active: &HashSet<SidebarAssetKey>) {
        if self.sidebar_asset_cache.len() <= MAX_SIDEBAR_ASSET_CACHE_ENTRIES {
            return;
        }
        let mut keys = self
            .sidebar_asset_cache
            .iter()
            .map(|(key, value)| (*key, active.contains(key), value.last_used))
            .collect::<Vec<_>>();
        keys.sort_unstable_by_key(|(_, is_active, last_used)| (*is_active, *last_used));
        let remove = self.sidebar_asset_cache.len() - MAX_SIDEBAR_ASSET_CACHE_ENTRIES;
        for (key, _, _) in keys.into_iter().take(remove) {
            self.sidebar_asset_cache.remove(&key);
        }
    }

    pub(super) fn compose(
        &mut self,
        frame: &mut FrameData,
        layout: ChromeLayout<'_>,
        cell: HostCellSize,
        palette: &Palette,
        occlusion: &surface::Occlusion,
    ) -> Vec<u8> {
        if !pixel_chrome_available(true, cell) {
            return self.cleanup();
        }
        let mut rows = Vec::with_capacity(layout.panes.len() * 2 + layout.decorations.len() + 3);
        if let Color::Rgb(or, og, ob) = palette.pane_gap_bg {
            let sidebar = layout
                .sidebar
                .into_iter()
                .map(|rect| (rect, palette.sidebar_bg));
            let panes = layout.panes.iter().map(|pane| {
                (
                    Rect::new(
                        layout.pane_area.x.saturating_add(pane.rect.x),
                        layout.pane_area.y.saturating_add(pane.rect.y),
                        pane.rect.width,
                        pane.rect.height,
                    ),
                    palette.pane_default_bg,
                )
            });
            for (rect, inside) in sidebar.chain(panes) {
                let Color::Rgb(ir, ig, ib) = inside else {
                    continue;
                };
                if rect.width < 2
                    || rect.height < 2
                    || rect.right() > frame.width
                    || rect.bottom() > frame.height
                {
                    continue;
                }
                for (edge, y, corner) in [
                    (Edge::Top, rect.y, "🭽"),
                    (Edge::Bottom, rect.bottom() - 1, "🭼"),
                ] {
                    let row = Rect::new(rect.x, y, rect.width, 1);
                    let corner_index =
                        usize::from(y) * usize::from(frame.width) + usize::from(rect.x);
                    let corner_cell = &frame.cells[corner_index];
                    if corner_cell.symbol != corner || occlusion.covers_rect(row) {
                        continue;
                    }
                    let Color::Rgb(r, g, b) = crate::protocol::u32_to_color(corner_cell.fg) else {
                        continue;
                    };
                    let title_span = (edge == Edge::Top)
                        .then(|| {
                            (1..rect.width.saturating_sub(1))
                                .filter(|column| {
                                    let cell = &frame.cells[usize::from(y)
                                        * usize::from(frame.width)
                                        + usize::from(rect.x + *column)];
                                    cell.bg == corner_cell.fg && cell.fg != corner_cell.fg
                                })
                                .fold(None, |span, column| {
                                    Some(match span {
                                        Some((start, _)) => (start, column + 1),
                                        None => (column, column + 1),
                                    })
                                })
                        })
                        .flatten();
                    rows.push(ChromeRow::Border(BorderRow {
                        rect: row,
                        edge,
                        cell_width: cell.width_px,
                        cell_height: cell.height_px,
                        inside: [ir, ig, ib],
                        outside: [or, og, ob],
                        stroke: [r, g, b],
                        title_span,
                    }));
                }
            }
        }
        if let (Some(rect), Color::Rgb(r, g, b)) = (layout.active_tab, palette.accent) {
            if !rect.is_empty()
                && rect
                    .x
                    .checked_add(rect.width)
                    .is_some_and(|right| right <= frame.width)
                && rect
                    .y
                    .checked_add(rect.height)
                    .is_some_and(|bottom| bottom <= frame.height)
                && !occlusion.covers_rect(rect)
            {
                rows.push(ChromeRow::TabUnderline(TabUnderline {
                    rect,
                    cell_width: cell.width_px,
                    cell_height: cell.height_px,
                    accent: [r, g, b],
                }));
            }
        }
        for decoration in layout.decorations {
            match decoration {
                SidebarDecoration::Icon(placement) => {
                    if placement.rect.width != 2
                        || placement.rect.height != 1
                        || placement.rect.right() > frame.width
                        || placement.rect.bottom() > frame.height
                        || occlusion.covers_rect(placement.rect)
                    {
                        continue;
                    }
                    let color = if placement.icon == crate::ui::SidebarIcon::GitBranch {
                        let origin = usize::from(placement.rect.y) * usize::from(frame.width)
                            + usize::from(placement.rect.x);
                        let branch_cell = &frame.cells[origin];
                        if branch_cell.modifier & Modifier::DIM.bits() != 0 {
                            continue;
                        }
                        let Some(color) = host_rgb(
                            crate::protocol::u32_to_color(branch_cell.fg),
                            layout.host_theme,
                            layout.host_theme.foreground,
                        ) else {
                            continue;
                        };
                        Some(color)
                    } else {
                        None
                    };
                    rows.push(ChromeRow::Icon(IconRow {
                        placement: *placement,
                        cell_width: cell.width_px,
                        cell_height: cell.height_px,
                        color,
                    }));
                }
                SidebarDecoration::Highlight(rect) => {
                    if rect.is_empty()
                        || rect
                            .x
                            .checked_add(rect.width)
                            .is_none_or(|right| right > frame.width)
                        || rect
                            .y
                            .checked_add(rect.height)
                            .is_none_or(|bottom| bottom > frame.height)
                        || occlusion.covers_rect(*rect)
                    {
                        continue;
                    }
                    let origin =
                        usize::from(rect.y) * usize::from(frame.width) + usize::from(rect.x);
                    let background = frame.cells[origin].bg;
                    let representable = (rect.y..rect.bottom()).all(|y| {
                        (rect.x..rect.right()).all(|x| {
                            let cell = &frame.cells
                                [usize::from(y) * usize::from(frame.width) + usize::from(x)];
                            cell.bg == background && cell.modifier & Modifier::REVERSED.bits() == 0
                        })
                    });
                    if !representable {
                        continue;
                    }
                    let background = crate::protocol::u32_to_color(background);
                    if background == Color::Reset {
                        continue;
                    }
                    let Some(fill) =
                        host_rgb(background, layout.host_theme, layout.host_theme.background)
                    else {
                        continue;
                    };
                    rows.push(ChromeRow::RoundedHighlight(RoundedHighlight {
                        rect: *rect,
                        cell_width: cell.width_px,
                        cell_height: cell.height_px,
                        fill,
                    }));
                }
            }
        }
        let changed = rows != self.rows;
        if changed {
            let mut scene = SurfaceGraphicsScene::default();
            let mut scene_assets = HashSet::new();
            let mut active_sidebar_assets = HashSet::new();
            for (index, row) in rows.iter().enumerate() {
                if let Some(key) = match row {
                    ChromeRow::Icon(row) => Some(row.asset_key()),
                    ChromeRow::RoundedHighlight(row) => Some(row.asset_key()),
                    ChromeRow::Border(_) | ChromeRow::TabUnderline(_) => None,
                } {
                    active_sidebar_assets.insert(key);
                }
                let data = match self.sidebar_asset_png(row) {
                    Ok(data) => data,
                    Err(error) => {
                        tracing::warn!(%error, "could not render pixel chrome");
                        return self.cleanup();
                    }
                };
                let rect = row.rect();
                let Some(image_width) = u32::from(rect.width).checked_mul(cell.width_px) else {
                    tracing::warn!("pixel chrome image width overflow");
                    return self.cleanup();
                };
                let Some(image_height) = u32::from(rect.height).checked_mul(cell.height_px) else {
                    tracing::warn!("pixel chrome image height overflow");
                    return self.cleanup();
                };
                let mut hash = std::collections::hash_map::DefaultHasher::new();
                data.hash(&mut hash);
                let key = SurfaceGraphicsAssetKey {
                    source: SurfaceGraphicsSource::PaneLayer {
                        pane_id: "herdr-chrome".into(),
                        layer_id: row.layer_id(index),
                    },
                    image_width,
                    image_height,
                    format: SurfaceGraphicsFormat::Png,
                    data_len: data.len() as u64,
                    data_fingerprint: hash.finish(),
                };
                let logical_placement_id = match row {
                    ChromeRow::Icon(_) | ChromeRow::RoundedHighlight(_) => {
                        u32::from(rect.y) << 16 | u32::from(rect.x)
                    }
                    ChromeRow::Border(_) | ChromeRow::TabUnderline(_) => 1,
                };
                scene.placements.push(SurfaceGraphicsPlacement {
                    asset: key.clone(),
                    logical_placement_id,
                    x: rect.x,
                    y: rect.y,
                    cols: u32::from(rect.width),
                    rows: u32::from(rect.height),
                    source_x: 0,
                    source_y: 0,
                    source_width: 0,
                    source_height: 0,
                    x_offset: 0,
                    y_offset: 0,
                    z: if matches!(row, ChromeRow::RoundedHighlight(_)) {
                        -2
                    } else {
                        -1
                    },
                    scrollback_offset: 0,
                });
                if scene_assets.insert(key.clone()) {
                    scene.assets.push(SurfaceGraphicsAsset { key, data });
                }
            }
            self.trim_sidebar_asset_cache(&active_sidebar_assets);
            self.graphics.set_scene(scene);
            self.rows = rows;
        }
        for row in &self.rows {
            let rect = row.rect();
            for y in rect.y..rect.bottom() {
                for x in rect.x..rect.right() {
                    let cell = &mut frame.cells
                        [usize::from(y) * usize::from(frame.width) + usize::from(x)];
                    match row {
                        ChromeRow::Border(border) => {
                            let column = x - rect.x;
                            let in_title = border
                                .title_span
                                .is_some_and(|(start, end)| (start..end).contains(&column));
                            if !in_title
                                && matches!(cell.symbol.as_str(), "🭽" | "🭾" | "🭼" | "🭿" | "▔" | "▁")
                            {
                                cell.symbol = " ".into();
                            }
                        }
                        ChromeRow::TabUnderline(_) => {
                            cell.modifier &= !Modifier::UNDERLINED.bits();
                        }
                        ChromeRow::Icon(icon)
                            if icon.placement.icon == crate::ui::SidebarIcon::GitBranch =>
                        {
                            cell.symbol = " ".into();
                        }
                        ChromeRow::RoundedHighlight(_) => {
                            cell.bg = crate::protocol::color_to_u32(palette.sidebar_bg);
                        }
                        ChromeRow::Icon(_) => {}
                    }
                }
            }
        }
        if changed {
            let update =
                self.graphics
                    .encode(surface::Visibility::Main, (0, 0), None, cell, occlusion);
            self.replay =
                self.graphics
                    .encode(surface::Visibility::Main, (0, 0), None, cell, occlusion);
            update
        } else {
            self.replay.clone()
        }
    }
}
