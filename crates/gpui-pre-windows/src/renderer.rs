//! The renderer of a window. The default is the wgpu renderer.
//! `GPUI_RENDERER=dx11` picks the Direct3D 11 renderer. A window whose wgpu
//! renderer fails to start uses the Direct3D 11 renderer too.

use std::{num::NonZeroIsize, rc::Rc, sync::Arc};

use anyhow::{Context as _, Result};
use gpui::{DevicePixels, GpuSpecs, PlatformAtlas, Scene, Size, WindowBackgroundAppearance};
use gpui_wgpu::{
    GpuContext, TextContrast, WgpuRenderer, WgpuSurfaceConfig, time_frame_draw, time_startup_stage,
};
use raw_window_handle as rwh;

use crate::{DirectXDevices, DirectXRenderer, HWND};

pub(crate) enum WindowRenderer {
    DirectX(DirectXRenderer),
    Wgpu(WgpuRenderer),
}

thread_local! {
    /// The windows of the thread share one wgpu device.
    static WGPU_CONTEXT: GpuContext = Rc::default();
}

fn wgpu_requested() -> bool {
    !std::env::var("GPUI_RENDERER").is_ok_and(|value| value == "dx11")
}

impl WindowRenderer {
    pub(crate) fn new(
        hwnd: HWND,
        directx_devices: &DirectXDevices,
        disable_direct_composition: bool,
        size: Size<DevicePixels>,
    ) -> Result<Self> {
        time_startup_stage("window-renderer", || {
            Self::new_inner(hwnd, directx_devices, disable_direct_composition, size)
        })
    }

    fn new_inner(
        hwnd: HWND,
        directx_devices: &DirectXDevices,
        disable_direct_composition: bool,
        size: Size<DevicePixels>,
    ) -> Result<Self> {
        if wgpu_requested() {
            match Self::new_wgpu(hwnd, size) {
                Ok(renderer) => return Ok(renderer),
                Err(error) => log::error!("{error:#}: falling back to Direct3D 11"),
            }
        }
        let renderer = DirectXRenderer::new(hwnd, directx_devices, disable_direct_composition)
            .context("Creating DirectX renderer")?;
        Ok(Self::DirectX(renderer))
    }

    fn new_wgpu(hwnd: HWND, size: Size<DevicePixels>) -> Result<Self> {
        // The window can be created with the default size of the system,
        // which is no real size. The first resize gives the real size.
        let valid = |side: DevicePixels| (1..=16384).contains(&side.0);
        let size = if valid(size.width) && valid(size.height) {
            size
        } else {
            Size::new(DevicePixels(1), DevicePixels(1))
        };
        let config = WgpuSurfaceConfig {
            size,
            transparent: false,
            preferred_present_mode: WgpuSurfaceConfig::present_mode_from_env(),
        };
        let context = WGPU_CONTEXT.with(Rc::clone);
        let mut renderer = WgpuRenderer::new(context, &RawWindow::new(hwnd), config, None)
            .context("Creating wgpu renderer")?;
        let font_info = DirectXRenderer::get_font_info();
        renderer.set_text_contrast(TextContrast {
            gamma: font_info.gamma,
            grayscale_enhanced_contrast: font_info.grayscale_enhanced_contrast,
            subpixel_enhanced_contrast: font_info.subpixel_enhanced_contrast,
        });
        renderer.set_subpixel_layout(font_info.is_bgr);
        Ok(Self::Wgpu(renderer))
    }

    /// Whether the renderer draws the custom surfaces of the scene.
    pub(crate) const fn draws_custom_surfaces(&self) -> bool {
        matches!(self, Self::Wgpu(_))
    }

    pub(crate) fn draw(
        &mut self,
        scene: &Scene,
        background_appearance: WindowBackgroundAppearance,
    ) -> Result<()> {
        time_frame_draw(|| match self {
            Self::DirectX(renderer) => renderer.draw(scene, background_appearance),
            Self::Wgpu(renderer) => {
                renderer.draw(scene);
                Ok(())
            }
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn render_to_image(
        &mut self,
        scene: &Scene,
        background_appearance: WindowBackgroundAppearance,
    ) -> Result<image::RgbaImage> {
        match self {
            Self::DirectX(renderer) => renderer.render_to_image(scene, background_appearance),
            Self::Wgpu(_) => anyhow::bail!("The wgpu renderer renders to no image"),
        }
    }

    pub(crate) fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        match self {
            Self::DirectX(renderer) => renderer.sprite_atlas(),
            Self::Wgpu(renderer) => renderer.sprite_atlas().clone(),
        }
    }

    pub(crate) fn gpu_specs(&self) -> Result<GpuSpecs> {
        match self {
            Self::DirectX(renderer) => renderer.gpu_specs(),
            Self::Wgpu(renderer) => renderer.gpu_specs().context("GPU resources released"),
        }
    }

    pub(crate) fn resize(&mut self, size: Size<DevicePixels>) -> Result<()> {
        match self {
            Self::DirectX(renderer) => renderer.resize(size),
            Self::Wgpu(renderer) => {
                renderer.update_drawable_size(size);
                Ok(())
            }
        }
    }

    pub(crate) fn handle_device_lost(&mut self, directx_devices: &DirectXDevices) -> Result<()> {
        match self {
            Self::DirectX(renderer) => renderer.handle_device_lost(directx_devices),
            // The wgpu renderer has its own device.
            Self::Wgpu(_) => Ok(()),
        }
    }

    pub(crate) fn mark_drawable(&mut self) {
        match self {
            Self::DirectX(renderer) => renderer.mark_drawable(),
            Self::Wgpu(_) => {}
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
