use std::hash::{Hash, Hasher};

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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ChromeRow {
    Border(BorderRow),
    TabUnderline(TabUnderline),
}

pub(super) struct ChromeLayout<'a> {
    pub(super) panes: &'a [PaneSurfacePane],
    pub(super) pane_area: Rect,
    pub(super) active_tab: Option<Rect>,
}

const CORNER_RADIUS_PX: u32 = 12;

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

impl ChromeRow {
    fn rect(&self) -> Rect {
        match self {
            Self::Border(row) => row.rect,
            Self::TabUnderline(row) => row.rect,
        }
    }

    fn png(&self) -> std::io::Result<Vec<u8>> {
        match self {
            Self::Border(row) => row.png(),
            Self::TabUnderline(row) => row.png(),
        }
    }

    fn layer_id(&self, index: usize) -> String {
        match self {
            Self::Border(_) => format!("border-{index}"),
            Self::TabUnderline(_) => format!("tab-underline-{index}"),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct PaneFrames {
    rows: Vec<ChromeRow>,
    graphics: surface::ClientState,
    replay: Vec<u8>,
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

    pub(super) fn compose(
        &mut self,
        frame: &mut FrameData,
        layout: ChromeLayout<'_>,
        cell: HostCellSize,
        palette: &Palette,
        occlusion: &surface::Occlusion,
    ) -> Vec<u8> {
        if !cell.is_known()
            || cell.height_px <= cell.width_px.div_ceil(2) + (cell.width_px / 8).max(1)
        {
            return self.cleanup();
        }
        let mut rows = Vec::with_capacity(layout.panes.len() * 2 + 1);
        if let (Color::Rgb(ir, ig, ib), Color::Rgb(or, og, ob)) =
            (palette.pane_default_bg, palette.pane_gap_bg)
        {
            for pane in layout.panes {
                let rect = Rect::new(
                    layout.pane_area.x.saturating_add(pane.rect.x),
                    layout.pane_area.y.saturating_add(pane.rect.y),
                    pane.rect.width,
                    pane.rect.height,
                );
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
        let changed = rows != self.rows;
        if changed {
            let mut scene = SurfaceGraphicsScene::default();
            for (index, row) in rows.iter().enumerate() {
                let data = match row.png() {
                    Ok(data) => data,
                    Err(error) => {
                        tracing::warn!(%error, "could not render pixel chrome");
                        return self.cleanup();
                    }
                };
                let mut hash = std::collections::hash_map::DefaultHasher::new();
                data.hash(&mut hash);
                let key = SurfaceGraphicsAssetKey {
                    source: SurfaceGraphicsSource::PaneLayer {
                        pane_id: "herdr-chrome".into(),
                        layer_id: row.layer_id(index),
                    },
                    image_width: u32::from(row.rect().width) * cell.width_px,
                    image_height: cell.height_px,
                    format: SurfaceGraphicsFormat::Png,
                    data_len: data.len() as u64,
                    data_fingerprint: hash.finish(),
                };
                scene.placements.push(SurfaceGraphicsPlacement {
                    asset: key.clone(),
                    logical_placement_id: 1,
                    x: row.rect().x,
                    y: row.rect().y,
                    cols: u32::from(row.rect().width),
                    rows: 1,
                    source_x: 0,
                    source_y: 0,
                    source_width: 0,
                    source_height: 0,
                    x_offset: 0,
                    y_offset: 0,
                    z: -1,
                    scrollback_offset: 0,
                });
                scene.assets.push(SurfaceGraphicsAsset { key, data });
            }
            self.graphics.set_scene(scene);
            self.rows = rows;
        }
        for row in &self.rows {
            let rect = row.rect();
            for x in rect.x..rect.right() {
                let cell = &mut frame.cells
                    [usize::from(rect.y) * usize::from(frame.width) + usize::from(x)];
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
