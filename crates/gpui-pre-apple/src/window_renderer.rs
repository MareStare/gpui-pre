//! The renderer of a window. The default is the wgpu renderer.
//! `GPUI_RENDERER=metal` picks the Metal renderer. A window whose wgpu
//! renderer fails to start uses the Metal renderer too.

use std::{ffi::c_void, ptr::NonNull, rc::Rc, sync::Arc};

use anyhow::Context as _;
#[cfg(any(test, feature = "test-support"))]
use anyhow::Result;
use foreign_types::ForeignType;
use gpui::{DevicePixels, PlatformAtlas, Scene, Size};
use gpui_wgpu::{
    GpuContext, TextContrast, WgpuRenderer, WgpuSurfaceConfig, time_frame_draw, time_startup_stage,
    wgpu, wgpu_requested,
};
#[cfg(any(test, feature = "test-support"))]
use image::RgbaImage;
use metal::CAMetalLayer;

use crate::metal_renderer::{Context, MetalRenderer, configure_window_layer};

pub enum Renderer {
    Metal(MetalRenderer),
    Wgpu(WgpuWindowRenderer),
}

/// A wgpu renderer that draws to the backing layer of a view.
pub struct WgpuWindowRenderer {
    layer: metal::MetalLayer,
    renderer: WgpuRenderer,
}

thread_local! {
    /// The windows of the thread share one wgpu device.
    static WGPU_CONTEXT: GpuContext = Rc::default();
}

/// Creates the renderer of a window. The view of the window takes the layer of the
/// renderer as its backing layer.
pub fn new_renderer(
    context: Context,
    bounds: Size<f32>,
    transparent: bool,
    use_display_p3: bool,
) -> Renderer {
    time_startup_stage("window-renderer", || {
        if wgpu_requested("metal") {
            match WgpuWindowRenderer::new(bounds, transparent, use_display_p3) {
                Ok(renderer) => return Renderer::Wgpu(renderer),
                Err(error) => log::error!("Using the Metal renderer: {error:#}"),
            }
        }
        Renderer::Metal(MetalRenderer::new(context, transparent, use_display_p3))
    })
}

impl WgpuWindowRenderer {
    fn new(bounds: Size<f32>, transparent: bool, use_display_p3: bool) -> anyhow::Result<Self> {
        let layer = metal::MetalLayer::new();
        configure_window_layer(&layer, transparent, use_display_p3);
        // The scale factor of the view is not known yet. The first resize gives the
        // drawable size.
        let side = |pixels: f32| DevicePixels((pixels as i32).clamp(1, 16384));
        let config = WgpuSurfaceConfig {
            size: Size::new(side(bounds.width), side(bounds.height)),
            transparent,
            preferred_present_mode: WgpuSurfaceConfig::present_mode_from_env(),
        };
        let layer_ptr = NonNull::new(layer.as_ptr().cast::<c_void>()).context("No CAMetalLayer")?;
        let context = WGPU_CONTEXT.with(Rc::clone);
        // wgpu sets the color space of the layer when it configures the surface.
        let color_space = if use_display_p3 {
            wgpu::SurfaceColorSpace::DisplayP3
        } else {
            wgpu::SurfaceColorSpace::Auto
        };
        // SAFETY: `layer_ptr` points to a `CAMetalLayer`.
        let mut renderer =
            unsafe { WgpuRenderer::new_from_metal_layer(context, layer_ptr, config, color_space) }
                .context("Creating wgpu renderer")?;
        // The Metal renderer blends Core Text glyphs with no gamma or contrast correction.
        renderer.set_text_contrast(TextContrast {
            gamma: 1.0,
            grayscale_enhanced_contrast: 0.0,
            subpixel_enhanced_contrast: 0.0,
        });
        Ok(Self { layer, renderer })
    }
}

impl Renderer {
    /// Whether the renderer draws the custom surfaces of the scene.
    pub const fn draws_custom_surfaces(&self) -> bool {
        matches!(self, Self::Wgpu(_))
    }

    pub fn layer(&self) -> Option<&metal::MetalLayerRef> {
        match self {
            Self::Metal(renderer) => renderer.layer(),
            Self::Wgpu(renderer) => Some(&renderer.layer),
        }
    }

    pub fn layer_ptr(&self) -> *mut CAMetalLayer {
        match self {
            Self::Metal(renderer) => renderer.layer_ptr(),
            Self::Wgpu(renderer) => renderer.layer.as_ptr(),
        }
    }

    pub fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        match self {
            Self::Metal(renderer) => renderer.sprite_atlas().clone(),
            Self::Wgpu(renderer) => renderer.renderer.sprite_atlas().clone(),
        }
    }

    pub fn set_presents_with_transaction(&mut self, presents_with_transaction: bool) {
        match self {
            Self::Metal(renderer) => {
                renderer.set_presents_with_transaction(presents_with_transaction)
            }
            // wgpu reads the flag of the layer at each frame and presents in the
            // transaction when it is set.
            Self::Wgpu(renderer) => renderer
                .layer
                .set_presents_with_transaction(presents_with_transaction),
        }
    }

    pub fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        match self {
            Self::Metal(renderer) => renderer.update_drawable_size(size),
            Self::Wgpu(renderer) => renderer.renderer.update_drawable_size(size),
        }
    }

    pub fn update_transparency(&mut self, transparent: bool) {
        match self {
            Self::Metal(renderer) => renderer.update_transparency(transparent),
            // The surface configuration sets the opacity of the layer.
            Self::Wgpu(renderer) => renderer.renderer.update_transparency(transparent),
        }
    }

    pub fn destroy(&mut self) {
        match self {
            Self::Metal(renderer) => renderer.destroy(),
            Self::Wgpu(renderer) => renderer.renderer.destroy(),
        }
    }

    pub fn draw(&mut self, scene: &Scene) {
        time_frame_draw(|| match self {
            Self::Metal(renderer) => renderer.draw(scene),
            Self::Wgpu(renderer) => {
                renderer.renderer.draw(scene);
            }
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn render_to_image(&mut self, scene: &Scene) -> Result<RgbaImage> {
        match self {
            Self::Metal(renderer) => renderer.render_to_image(scene),
            Self::Wgpu(_) => anyhow::bail!("The wgpu renderer renders to no image"),
        }
    }
}
