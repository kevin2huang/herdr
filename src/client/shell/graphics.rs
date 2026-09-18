use super::*;

impl ClientShellState {
    #[cfg(unix)]
    pub(crate) fn graphics_scope(&self) -> &str {
        self.graphics.scope()
    }

    #[cfg(unix)]
    pub(crate) fn trust_direct_graphics_asset(
        &mut self,
        key: &crate::protocol::SurfaceGraphicsAssetKey,
        image_id: u32,
    ) -> bool {
        self.graphics.trust_direct_asset(key, image_id)
    }

    #[cfg(unix)]
    pub(crate) fn retire_direct_graphics_image(&mut self, image_id: u32) {
        self.graphics.retire_direct_image(image_id);
    }

    pub(crate) fn take_pending_graphics_cleanup(&mut self) -> Vec<u8> {
        let mut bytes = self.graphics.take_pending_cleanup();
        bytes.extend(self.pane_frames.take_pending_cleanup());
        bytes
    }

    pub(crate) fn set_graphics_cell_size(&mut self, width_px: u32, height_px: u32) {
        self.graphics_cell_size = crate::kitty_graphics::HostCellSize {
            width_px: width_px.max(1),
            height_px: height_px.max(1),
        };
    }

    pub(super) fn compose_unavailable_graphics(
        &mut self,
        frame: &mut FrameData,
        sidebar: Option<Rect>,
        occlusion: &crate::kitty_graphics::surface::Occlusion,
    ) {
        frame.graphics = self.graphics.encode(
            crate::kitty_graphics::surface::Visibility::Hidden,
            (0, 0),
            None,
            self.graphics_cell_size,
            occlusion,
        );
        if !self.config.pixel_pane_borders || sidebar.is_none() {
            frame.graphics.extend(self.pane_frames.cleanup());
            return;
        }
        let chrome = self.pane_frames.compose(
            frame,
            pane_frames::ChromeLayout {
                panes: &[],
                pane_area: Rect::default(),
                sidebar,
                active_tab: None,
            },
            self.graphics_cell_size,
            &self.config.palette,
            occlusion,
        );
        frame.graphics.extend(chrome);
    }

    pub(super) fn compose_graphics(
        &mut self,
        frame: &mut FrameData,
        layout: ClientShellLayout,
        occlusion: &crate::kitty_graphics::surface::Occlusion,
    ) {
        let visibility = if self.endpoint_error.is_some() {
            crate::kitty_graphics::surface::Visibility::Hidden
        } else if self.hits.popup.is_some() {
            crate::kitty_graphics::surface::Visibility::Popup
        } else {
            crate::kitty_graphics::surface::Visibility::Main
        };
        let popup_origin = self
            .hits
            .popup
            .as_ref()
            .map(|popup| (popup.inner_rect.x, popup.inner_rect.y));
        frame.graphics = self.graphics.encode(
            visibility,
            (layout.pane_surface.x, layout.pane_surface.y),
            popup_origin,
            self.graphics_cell_size,
            occlusion,
        );
        if !self.config.pixel_pane_borders {
            frame.graphics.extend(self.pane_frames.cleanup());
            return;
        }
        let panes = if self.endpoint_error.is_some() {
            &[][..]
        } else {
            self.pane_surface
                .as_ref()
                .map_or(&[][..], |surface| surface.panes.as_slice())
        };
        let active_tab = self
            .endpoint_error
            .is_none()
            .then_some(self.snapshot.as_deref())
            .flatten()
            .and_then(|snapshot| snapshot.focused_tab_id.as_deref())
            .and_then(|focused| {
                self.hits
                    .tabs
                    .iter()
                    .find_map(|(rect, tab_id)| (tab_id == focused).then_some(*rect))
            });
        let chrome_graphics = self.pane_frames.compose(
            frame,
            pane_frames::ChromeLayout {
                panes,
                pane_area: layout.pane_surface,
                sidebar: (!layout.sidebar.is_empty()).then_some(layout.sidebar),
                active_tab,
            },
            self.graphics_cell_size,
            &self.config.palette,
            occlusion,
        );
        frame.graphics.extend(chrome_graphics);
    }
}
