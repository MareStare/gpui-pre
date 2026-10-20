//! Custom surfaces: code outside the renderer draws into the frame at the
//! place of a surface in the scene.

use std::sync::Arc;

use gpui::{Bounds, CustomSurface, DevicePixels, ScaledPixels, Size};

/// Draws a custom surface into the frame of the wgpu renderer.
pub trait WgpuSurface: Send + Sync + 'static {
    /// Records the commands that draw the surface. The target holds the
    /// frame up to the surface. No render pass is open.
    fn draw(&self, frame: WgpuSurfaceFrame<'_>);
}

/// The frame that a custom surface draws into.
pub struct WgpuSurfaceFrame<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub target: &'a wgpu::TextureView,
    pub target_format: wgpu::TextureFormat,
    pub target_size: Size<DevicePixels>,

    /// The bounds of the surface in device pixels.
    pub bounds: Bounds<ScaledPixels>,

    /// The visible part of the window in device pixels.
    pub content_mask: Bounds<ScaledPixels>,
}

/// The content of a custom surface that the wgpu renderer draws.
pub fn wgpu_custom_surface(surface: Arc<dyn WgpuSurface>) -> CustomSurface {
    Arc::new(surface)
}

pub(crate) fn as_wgpu_surface(content: &CustomSurface) -> Option<&Arc<dyn WgpuSurface>> {
    content.downcast_ref()
}
