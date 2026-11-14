//! The wgpu renderer of a native window, for the platforms that also have a native
//! renderer. The windows of a thread share one device.

use std::rc::Rc;
use std::time::Duration;

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

/// The delays between the tries to make a lost device again. Each failed try
/// doubles the delay, up to a limit. The delays keep a window from a busy loop
/// while the GPU is gone.
#[derive(Default)]
struct RecoveryBackoff {
    last_delay: Option<Duration>,
}

impl RecoveryBackoff {
    const FIRST_DELAY: Duration = Duration::from_millis(100);
    const MAX_DELAY: Duration = Duration::from_secs(2);

    /// Records a failed try. Returns the delay until the next try.
    fn failed(&mut self) -> Duration {
        let delay = self
            .last_delay
            .map_or(Self::FIRST_DELAY, |delay| (delay * 2).min(Self::MAX_DELAY));
        self.last_delay = Some(delay);
        delay
    }

    fn succeeded(&mut self) {
        self.last_delay = None;
    }
}

pub struct WgpuWindowRenderer {
    renderer: WgpuRenderer,
    source: Box<dyn SurfaceSource>,
    force_render: bool,
    recovery: RecoveryBackoff,
    recovery_retry: Option<Duration>,
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
        let side = |side: DevicePixels| DevicePixels(side.0.clamp(1, MAX_SIDE));
        let config = WgpuSurfaceConfig {
            size: size.map(side),
            transparent,
            preferred_present_mode: WgpuSurfaceConfig::present_mode_from_env(),
        };
        let context = WINDOW_CONTEXT.with(Rc::clone);
        let renderer = WgpuRenderer::new_from_source(context, &*source, config, color_space, None)
            .context("Creating wgpu renderer")?;
        Ok(Self {
            renderer,
            source,
            force_render: false,
            recovery: RecoveryBackoff::default(),
            recovery_retry: None,
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
    /// the scene again: see `take_force_render`. When it cannot make the
    /// device, it tells the window when to try again: see
    /// `take_recovery_retry`.
    pub fn draw(&mut self, scene: &Scene) {
        if self.renderer.device_lost() {
            match self.renderer.recover_from(&*self.source) {
                Ok(()) => {
                    self.recovery.succeeded();
                    self.force_render = true;
                }
                Err(error) => {
                    let delay = self.recovery.failed();
                    log::warn!("GPU recovery failed, will retry in {delay:?}: {error:#}");
                    self.recovery_retry = Some(delay);
                }
            }
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

    /// The delay after which the window must force a render, so that the
    /// renderer tries again to make its lost device. Clears the delay.
    pub fn take_recovery_retry(&mut self) -> Option<Duration> {
        self.recovery_retry.take()
    }

    pub fn render_to_image(&mut self, scene: &Scene) -> Result<image::RgbaImage> {
        self.renderer.render_to_image(scene)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::RecoveryBackoff;

    #[test]
    fn failed_recoveries_wait_longer_up_to_a_limit() {
        let mut backoff = RecoveryBackoff::default();
        let delays: Vec<_> = (0..7).map(|_| backoff.failed()).collect();
        let ms = Duration::from_millis;
        assert_eq!(
            delays,
            [
                ms(100),
                ms(200),
                ms(400),
                ms(800),
                ms(1600),
                ms(2000),
                ms(2000)
            ]
        );
        backoff.succeeded();
        assert_eq!(backoff.failed(), ms(100), "a success resets the delay");
    }
}
