//! The wgpu renderer of a native window, for the platforms that also have a native
//! renderer. The windows of a thread share one device.

use std::rc::Rc;

use anyhow::{Context as _, Result};
use gpui::{DevicePixels, Scene, Size};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::wgpu_renderer::{SurfaceSource, WindowSource};
use crate::{GpuContext, WgpuRenderer, WgpuSurfaceConfig};

thread_local! {
    static WINDOW_CONTEXT: GpuContext = Rc::default();
}

/// The largest side of a window surface.
const MAX_SIDE: i32 = 16384;

pub struct WgpuWindowRenderer {
    renderer: WgpuRenderer,
    source: Box<dyn SurfaceSource>,
    force_render: bool,
}

impl WgpuWindowRenderer {
    /// Creates the renderer of a window. `size` can be a size that the window does
    /// not have yet, e.g. a default size of the system. The first resize gives the
    /// real size.
    pub fn new<W>(window: W, size: Size<DevicePixels>, transparent: bool) -> Result<Self>
    where
        W: HasWindowHandle + HasDisplayHandle + std::fmt::Debug + Send + Sync + Clone + 'static,
    {
        Self::from_source(Box::new(WindowSource(window)), size, transparent)
    }

    /// Creates a renderer that draws to a `CAMetalLayer`, e.g. the backing layer of
    /// an AppKit view. `size` is as for `new`.
    ///
    /// # Safety
    /// `layer` must point to a `CAMetalLayer` that lives as long as the renderer.
    #[cfg(target_vendor = "apple")]
    pub unsafe fn new_metal_layer(
        layer: std::ptr::NonNull<std::ffi::c_void>,
        size: Size<DevicePixels>,
        transparent: bool,
    ) -> Result<Self> {
        let source = crate::wgpu_renderer::MetalLayerSource(layer);
        Self::from_source(Box::new(source), size, transparent)
    }

    fn from_source(
        source: Box<dyn SurfaceSource>,
        size: Size<DevicePixels>,
        transparent: bool,
    ) -> Result<Self> {
        let side = |side: DevicePixels| DevicePixels(side.0.clamp(1, MAX_SIDE));
        let config = WgpuSurfaceConfig {
            size: size.map(side),
            transparent,
            preferred_present_mode: WgpuSurfaceConfig::present_mode_from_env(),
        };
        let context = WINDOW_CONTEXT.with(Rc::clone);
        let renderer = WgpuRenderer::new_from_source(context, &*source, config, None)
            .context("Creating wgpu renderer")?;
        Ok(Self {
            renderer,
            source,
            force_render: false,
        })
    }

    pub fn renderer(&self) -> &WgpuRenderer {
        &self.renderer
    }

    pub fn renderer_mut(&mut self) -> &mut WgpuRenderer {
        &mut self.renderer
    }

    /// Draws the scene. After a lost device, it makes a new device and draws
    /// nothing. The new device has an empty atlas, so the next frame must build
    /// the scene again: see `take_force_render`.
    pub fn draw(&mut self, scene: &Scene) {
        if self.renderer.device_lost() {
            if let Err(error) = self.renderer.recover_from(&*self.source) {
                log::warn!("GPU recovery failed, will retry on next frame: {error:#}");
            }
            self.force_render = true;
            return;
        }
        self.renderer.draw(scene);
        if self.renderer.needs_redraw() {
            self.force_render = true;
        }
    }

    /// Tells whether the next frame must build the scene again, and clears the flag.
    pub fn take_force_render(&mut self) -> bool {
        std::mem::take(&mut self.force_render)
    }

    pub fn render_to_image<T>(&self) -> Result<T> {
        anyhow::bail!("The wgpu window renderer renders to no image")
    }
}
