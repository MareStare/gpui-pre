//! The renderer of a native window, for the platforms that also have a native
//! renderer. The default is wgpu. `GPUI_RENDERER` can pick the native renderer, and
//! a window whose wgpu renderer fails to start uses the native renderer too. The wgpu
//! renderers of a thread share one device.

use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use gpui::{DevicePixels, GpuFailure, PlatformAtlas, Scene, Size};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::renderer_choice::wgpu_requested;
use crate::startup_log::{StartedRenderer, time_window_renderer};
use crate::wgpu_renderer::{SurfaceSource, WindowSource};
use crate::{GpuContext, WgpuRenderer, WgpuSurfaceConfig};

/// The native renderer of a platform.
pub trait NativeRenderer {
    /// The value of `GPUI_RENDERER` that picks the renderer. The startup log names the
    /// renderer `native:<NAME>`.
    const NAME: &'static str;

    /// What the wgpu renderer of the platform draws to. It lives as long as the wgpu
    /// renderer.
    type WgpuTarget;

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas>;

    /// Resizes the drawable of the window.
    fn resize(&mut self, size: Size<DevicePixels>) -> Result<()>;

    /// Sets whether the window shows what is behind it. The default does nothing, for a
    /// renderer that takes the background at each frame.
    fn update_transparency(&mut self, _transparent: bool) {}
}

/// The renderer of a window: wgpu, or the native renderer of the platform.
pub enum WindowRenderer<N: NativeRenderer> {
    Wgpu {
        renderer: WgpuWindowRenderer,

        /// Drops after `renderer`.
        target: N::WgpuTarget,
    },
    Native(N),
}

impl<N: NativeRenderer> WindowRenderer<N> {
    /// Starts the renderer of a window, and logs the time of the start. It starts the
    /// wgpu renderer with `new_wgpu`, unless `GPUI_RENDERER` picks the native renderer.
    /// It starts the native renderer with `new_native` in that case, and when the wgpu
    /// renderer fails to start.
    pub fn start(
        new_wgpu: impl FnOnce() -> Result<(WgpuWindowRenderer, N::WgpuTarget)>,
        new_native: impl FnOnce() -> Result<N>,
    ) -> Result<Self> {
        time_window_renderer(
            || {
                if wgpu_requested(N::NAME) {
                    match new_wgpu() {
                        Ok((renderer, target)) => return Ok(Self::Wgpu { renderer, target }),
                        Err(error) => log::error!("Using the {} renderer: {error:#}", N::NAME),
                    }
                }
                new_native().map(Self::Native)
            },
            |renderer| match renderer {
                Self::Wgpu { renderer, .. } => {
                    StartedRenderer::Wgpu(renderer.renderer().backend_name())
                }
                Self::Native(_) => StartedRenderer::Native(N::NAME),
            },
        )
    }

    pub fn native(&self) -> Option<&N> {
        match self {
            Self::Wgpu { .. } => None,
            Self::Native(renderer) => Some(renderer),
        }
    }

    pub fn native_mut(&mut self) -> Option<&mut N> {
        match self {
            Self::Wgpu { .. } => None,
            Self::Native(renderer) => Some(renderer),
        }
    }

    /// Whether the renderer draws the custom surfaces of the scene.
    pub fn draws_custom_surfaces(&self) -> bool {
        matches!(self, Self::Wgpu { .. })
    }

    /// Tells whether the next frame must build the scene again, and clears the flag.
    pub fn take_force_render(&mut self) -> bool {
        match self {
            Self::Wgpu { renderer, .. } => renderer.take_force_render(),
            Self::Native(_) => false,
        }
    }

    /// The GPU error that stays after the wgpu renderer dropped its GPU caches to
    /// recover. A native renderer has none.
    pub fn gpu_failure(&self) -> Option<GpuFailure> {
        match self {
            Self::Wgpu { renderer, .. } => renderer.renderer().gpu_failure(),
            Self::Native(_) => None,
        }
    }

    pub fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        match self {
            Self::Wgpu { renderer, .. } => renderer.renderer().sprite_atlas().clone(),
            Self::Native(renderer) => renderer.sprite_atlas(),
        }
    }

    pub fn resize(&mut self, size: Size<DevicePixels>) -> Result<()> {
        match self {
            Self::Wgpu { renderer, .. } => {
                renderer.renderer_mut().update_drawable_size(size);
                Ok(())
            }
            Self::Native(renderer) => renderer.resize(size),
        }
    }

    /// Sets whether the window shows what is behind it. The wgpu renderer sets the alpha
    /// mode of the surface. A surface that offers only an opaque alpha mode stays opaque.
    pub fn update_transparency(&mut self, transparent: bool) {
        match self {
            Self::Wgpu { renderer, .. } => renderer.renderer_mut().update_transparency(transparent),
            Self::Native(renderer) => renderer.update_transparency(transparent),
        }
    }
}

thread_local! {
    static WINDOW_CONTEXT: GpuContext = Rc::default();
}

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
        Self::from_source(
            Box::new(WindowSource(window)),
            size,
            transparent,
            wgpu::SurfaceColorSpace::Auto,
        )
    }

    /// Creates a renderer that draws to a `CAMetalLayer`, e.g. the backing layer of
    /// an AppKit view. `size` is as for `new`. wgpu sets `color_space` on the layer.
    ///
    /// # Safety
    /// `layer` must point to a `CAMetalLayer` that lives as long as the renderer.
    #[cfg(target_vendor = "apple")]
    pub unsafe fn new_metal_layer(
        layer: std::ptr::NonNull<std::ffi::c_void>,
        size: Size<DevicePixels>,
        transparent: bool,
        color_space: wgpu::SurfaceColorSpace,
    ) -> Result<Self> {
        let source = crate::wgpu_renderer::MetalLayerSource(layer);
        Self::from_source(Box::new(source), size, transparent, color_space)
    }

    fn from_source(
        source: Box<dyn SurfaceSource>,
        size: Size<DevicePixels>,
        transparent: bool,
        color_space: wgpu::SurfaceColorSpace,
    ) -> Result<Self> {
        let config = WgpuSurfaceConfig {
            size,
            transparent,
            preferred_present_mode: None,
        };
        let context = WINDOW_CONTEXT.with(Rc::clone);
        let renderer = WgpuRenderer::new_from_source(context, &*source, config, color_space, None)
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

    pub fn render_to_pending_image(&mut self, scene: &Scene) -> Result<gpui::PendingImage> {
        self.renderer.render_to_pending_image(scene)
    }
}
