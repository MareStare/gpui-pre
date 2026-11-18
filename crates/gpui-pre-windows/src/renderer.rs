//! The renderer of a window. The default is the wgpu renderer.
//! `GPUI_RENDERER=dx11` picks the Direct3D 11 renderer. A window whose wgpu
//! renderer fails to start uses the Direct3D 11 renderer too.

use std::{num::NonZeroIsize, sync::Arc};

use anyhow::{Context as _, Result};
use gpui::{DevicePixels, GpuSpecs, PlatformAtlas, Scene, Size, WindowBackgroundAppearance};
use gpui_wgpu::{NativeRenderer, TextContrast, WgpuWindowRenderer};
use raw_window_handle as rwh;

use crate::{DirectXDevices, DirectXRenderer, HWND};

pub(crate) type WindowRenderer = gpui_wgpu::WindowRenderer<DirectXRenderer>;

impl NativeRenderer for DirectXRenderer {
    const NAME: &'static str = "dx11";

    /// The wgpu renderer draws to the window itself.
    type WgpuTarget = ();

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        DirectXRenderer::sprite_atlas(self)
    }
}

pub(crate) fn new_window_renderer(
    hwnd: HWND,
    directx_devices: &DirectXDevices,
    disable_direct_composition: bool,
    size: Size<DevicePixels>,
) -> Result<WindowRenderer> {
    WindowRenderer::start(
        || Ok((new_wgpu(hwnd, size)?, ())),
        || {
            DirectXRenderer::new(hwnd, directx_devices, disable_direct_composition)
                .context("Creating DirectX renderer")
        },
    )
}

fn new_wgpu(hwnd: HWND, size: Size<DevicePixels>) -> Result<WgpuWindowRenderer> {
    // The window can be created with the default size of the system,
    // which is no real size. The first resize gives the real size.
    let mut window = WgpuWindowRenderer::new(RawWindow::new(hwnd), size, false)?;
    let renderer = window.renderer_mut();
    let font_info = DirectXRenderer::get_font_info();
    renderer.set_text_contrast(TextContrast {
        gamma: font_info.gamma,
        grayscale_enhanced_contrast: font_info.grayscale_enhanced_contrast,
        subpixel_enhanced_contrast: font_info.subpixel_enhanced_contrast,
    });
    renderer.set_subpixel_layout(font_info.is_bgr);
    Ok(window)
}

/// The operations of a window renderer that only Windows has.
pub(crate) trait WindowRendererExt {
    fn draw(
        &mut self,
        scene: &Scene,
        background_appearance: WindowBackgroundAppearance,
    ) -> Result<()>;
    fn render_to_pending_image(
        &mut self,
        scene: &Scene,
        background_appearance: WindowBackgroundAppearance,
    ) -> Result<gpui::PendingImage>;
    fn gpu_specs(&self) -> Result<GpuSpecs>;
    fn update_transparency(&mut self, transparent: bool);
    fn resize(&mut self, size: Size<DevicePixels>) -> Result<()>;
    fn handle_device_lost(&mut self, directx_devices: &DirectXDevices) -> Result<()>;
    fn mark_drawable(&mut self);
}

impl WindowRendererExt for WindowRenderer {
    fn draw(
        &mut self,
        scene: &Scene,
        background_appearance: WindowBackgroundAppearance,
    ) -> Result<()> {
        match self {
            Self::Wgpu { renderer, .. } => {
                renderer.draw(scene);
                Ok(())
            }
            Self::Native(renderer) => renderer.draw(scene, background_appearance),
        }
    }

    fn render_to_pending_image(
        &mut self,
        scene: &Scene,
        background_appearance: WindowBackgroundAppearance,
    ) -> Result<gpui::PendingImage> {
        match self {
            Self::Wgpu { renderer, .. } => renderer.render_to_pending_image(scene),
            Self::Native(renderer) => renderer
                .render_to_image(scene, background_appearance)
                .map(gpui::PendingImage::ready),
        }
    }

    fn gpu_specs(&self) -> Result<GpuSpecs> {
        match self {
            Self::Wgpu { renderer, .. } => renderer
                .renderer()
                .gpu_specs()
                .context("GPU resources released"),
            Self::Native(renderer) => renderer.gpu_specs(),
        }
    }

    fn update_transparency(&mut self, transparent: bool) {
        match self {
            // The swapchain of a window handle often offers only an opaque alpha mode.
            // The window then stays opaque.
            Self::Wgpu { renderer, .. } => renderer.renderer_mut().update_transparency(transparent),
            // The renderer takes the background appearance at each frame.
            Self::Native(_) => {}
        }
    }

    fn resize(&mut self, size: Size<DevicePixels>) -> Result<()> {
        match self {
            Self::Wgpu { renderer, .. } => {
                renderer.renderer_mut().update_drawable_size(size);
                Ok(())
            }
            Self::Native(renderer) => renderer.resize(size),
        }
    }

    fn handle_device_lost(&mut self, directx_devices: &DirectXDevices) -> Result<()> {
        match self {
            // The wgpu renderer has its own device and recovers it in `draw`.
            Self::Wgpu { .. } => Ok(()),
            Self::Native(renderer) => renderer.handle_device_lost(directx_devices),
        }
    }

    fn mark_drawable(&mut self) {
        match self {
            Self::Wgpu { .. } => {}
            Self::Native(renderer) => renderer.mark_drawable(),
        }
    }
}

/// The window handle that the wgpu surface keeps.
#[derive(Clone, Copy, Debug)]
struct RawWindow {
    hwnd: isize,
    hinstance: isize,
}

impl RawWindow {
    fn new(hwnd: HWND) -> Self {
        Self {
            hwnd: hwnd.0 as isize,
            hinstance: crate::get_module_handle().0 as isize,
        }
    }
}

impl rwh::HasWindowHandle for RawWindow {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        let hwnd = NonZeroIsize::new(self.hwnd).ok_or(rwh::HandleError::Unavailable)?;
        let mut handle = rwh::Win32WindowHandle::new(hwnd);
        // Vulkan needs the module of the window.
        handle.hinstance = NonZeroIsize::new(self.hinstance);
        let raw = handle.into();
        // SAFETY: the window outlives its renderer.
        Ok(unsafe { rwh::WindowHandle::borrow_raw(raw) })
    }
}

impl rwh::HasDisplayHandle for RawWindow {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        Ok(rwh::DisplayHandle::windows())
    }
}
