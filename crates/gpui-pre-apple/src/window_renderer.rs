//! The renderer of a window. The default is the wgpu renderer.
//! `GPUI_RENDERER=metal` picks the Metal renderer. A window whose wgpu
//! renderer fails to start uses the Metal renderer too.

use std::{ffi::c_void, ptr::NonNull, sync::Arc};

use anyhow::Context as _;
#[cfg(any(test, feature = "test-support"))]
use anyhow::Result;
use foreign_types::ForeignType;
use gpui::{DevicePixels, PlatformAtlas, Scene, Size};
use gpui_wgpu::{
    TextContrast, WgpuWindowRenderer, time_frame_draw, time_startup_stage, wgpu_requested,
};
#[cfg(any(test, feature = "test-support"))]
use image::RgbaImage;
use metal::CAMetalLayer;

use crate::metal_renderer::{Context, MetalRenderer, configure_window_layer};

pub enum Renderer {
    Metal(MetalRenderer),
    Wgpu(WgpuLayerRenderer),
}

/// A wgpu renderer that draws to the backing layer of a view.
pub struct WgpuLayerRenderer {
    renderer: WgpuWindowRenderer,
    layer: metal::MetalLayer,
}

/// Creates the renderer of a window. The view of the window takes the layer of the
/// renderer as its backing layer.
pub fn new_renderer(context: Context, bounds: Size<f32>, transparent: bool) -> Renderer {
    time_startup_stage("window-renderer", || {
        if wgpu_requested("metal") {
            match WgpuLayerRenderer::new(bounds, transparent) {
                Ok(renderer) => return Renderer::Wgpu(renderer),
                Err(error) => log::error!("Using the Metal renderer: {error:#}"),
            }
        }
        Renderer::Metal(MetalRenderer::new(context, transparent))
    })
}

impl WgpuLayerRenderer {
    fn new(bounds: Size<f32>, transparent: bool) -> anyhow::Result<Self> {
        let layer = metal::MetalLayer::new();
        configure_window_layer(&layer, transparent);
        // The scale factor of the view is not known yet. The first resize gives the
        // drawable size.
        let size = bounds.map(|pixels| DevicePixels(pixels as i32));
        let layer_ptr = NonNull::new(layer.as_ptr().cast::<c_void>()).context("No CAMetalLayer")?;
        // SAFETY: `layer_ptr` points to `layer`, a `CAMetalLayer`. The renderer is
        // dropped before `layer`.
        let mut renderer =
            unsafe { WgpuWindowRenderer::new_metal_layer(layer_ptr, size, transparent) }?;
        // The Metal renderer blends Core Text glyphs with no gamma or contrast correction.
        renderer.renderer_mut().set_text_contrast(TextContrast {
            gamma: 1.0,
            grayscale_enhanced_contrast: 0.0,
            subpixel_enhanced_contrast: 0.0,
        });
        Ok(Self { renderer, layer })
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
            Self::Wgpu(renderer) => renderer.renderer.renderer().sprite_atlas().clone(),
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
            Self::Wgpu(renderer) => renderer.renderer.renderer_mut().update_drawable_size(size),
        }
    }

    pub fn update_transparency(&mut self, transparent: bool) {
        match self {
            Self::Metal(renderer) => renderer.update_transparency(transparent),
            // The surface configuration sets the opacity of the layer.
            Self::Wgpu(renderer) => renderer
                .renderer
                .renderer_mut()
                .update_transparency(transparent),
        }
    }

    pub fn destroy(&mut self) {
        match self {
            Self::Metal(renderer) => renderer.destroy(),
            Self::Wgpu(renderer) => renderer.renderer.renderer_mut().destroy(),
        }
    }

    pub fn draw(&mut self, scene: &Scene) {
        time_frame_draw(|| match self {
            Self::Metal(renderer) => renderer.draw(scene),
            Self::Wgpu(renderer) => renderer.renderer.draw(scene),
        })
    }

    /// Tells whether the next frame must build the scene again, and clears the flag.
    pub fn take_force_render(&mut self) -> bool {
        match self {
            Self::Metal(_) => false,
            Self::Wgpu(renderer) => renderer.renderer.take_force_render(),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn render_to_image(&mut self, scene: &Scene) -> Result<RgbaImage> {
        match self {
            Self::Metal(renderer) => renderer.render_to_image(scene),
            Self::Wgpu(renderer) => renderer.renderer.render_to_image(),
        }
    }
}
