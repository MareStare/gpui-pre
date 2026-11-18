//! The renderer of a window. The default is the wgpu renderer.
//! `GPUI_RENDERER=metal` picks the Metal renderer. A window whose wgpu
//! renderer fails to start uses the Metal renderer too.

use std::{ffi::c_void, ptr::NonNull, sync::Arc};

use anyhow::Result;
use gpui::{DevicePixels, PlatformAtlas, Scene, Size};
use gpui_wgpu::{NativeRenderer, TextContrast, WgpuWindowRenderer, WindowRenderer, wgpu};
use objc2::rc::Retained;
use objc2_quartz_core::CAMetalLayer;

use crate::metal_renderer::{Context, MetalRenderer, configure_window_layer};

pub type Renderer = WindowRenderer<MetalRenderer>;

impl NativeRenderer for MetalRenderer {
    const NAME: &'static str = "metal";

    /// The backing layer of the view.
    type WgpuTarget = Retained<CAMetalLayer>;

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        MetalRenderer::sprite_atlas(self).clone()
    }
}

/// Creates the renderer of a window. The view of the window takes the layer of the
/// renderer as its backing layer.
pub fn new_renderer(
    context: Context,
    bounds: Size<f32>,
    transparent: bool,
    use_display_p3: bool,
) -> Result<Renderer> {
    Renderer::start(
        || new_wgpu(bounds, transparent, use_display_p3),
        || MetalRenderer::new(context, transparent, use_display_p3),
    )
}

/// Creates a wgpu renderer that draws to a layer for the view.
fn new_wgpu(
    bounds: Size<f32>,
    transparent: bool,
    use_display_p3: bool,
) -> Result<(WgpuWindowRenderer, Retained<CAMetalLayer>)> {
    let layer = CAMetalLayer::new();
    configure_window_layer(&layer, transparent, use_display_p3);
    // The scale factor of the view is not known yet. The first resize gives the
    // drawable size.
    let size = bounds.map(|pixels| DevicePixels(pixels as i32));
    let layer_ptr = NonNull::from(&*layer).cast::<c_void>();
    // wgpu sets the color space of the layer when it configures the surface.
    let color_space = if use_display_p3 {
        wgpu::SurfaceColorSpace::DisplayP3
    } else {
        wgpu::SurfaceColorSpace::Auto
    };
    // SAFETY: `layer_ptr` points to `layer`, a `CAMetalLayer`. The window renderer
    // drops the wgpu renderer before `layer`.
    let mut renderer =
        unsafe { WgpuWindowRenderer::new_metal_layer(layer_ptr, size, transparent, color_space) }?;
    // The Metal renderer blends Core Text glyphs with no gamma or contrast correction.
    renderer.renderer_mut().set_text_contrast(TextContrast {
        gamma: 1.0,
        grayscale_enhanced_contrast: 0.0,
        subpixel_enhanced_contrast: 0.0,
    });
    Ok((renderer, layer))
}

/// The operations of a window renderer that only macOS has.
pub trait WindowRendererExt {
    fn layer(&self) -> Option<&CAMetalLayer>;
    fn layer_ptr(&self) -> *mut CAMetalLayer;
    fn set_presents_with_transaction(&mut self, presents_with_transaction: bool);
    fn update_drawable_size(&mut self, size: Size<DevicePixels>);
    fn update_transparency(&mut self, transparent: bool);
    fn destroy(&mut self);
    fn draw(&mut self, scene: &Scene);
    fn render_to_pending_image(&mut self, scene: &Scene) -> Result<gpui::PendingImage>;
}

impl WindowRendererExt for Renderer {
    fn layer(&self) -> Option<&CAMetalLayer> {
        match self {
            Self::Wgpu { target, .. } => Some(target),
            Self::Native(renderer) => renderer.layer(),
        }
    }

    fn layer_ptr(&self) -> *mut CAMetalLayer {
        match self {
            Self::Wgpu { target, .. } => Retained::as_ptr(target).cast_mut(),
            Self::Native(renderer) => renderer.layer_ptr(),
        }
    }

    fn set_presents_with_transaction(&mut self, presents_with_transaction: bool) {
        match self {
            // wgpu reads the flag of the layer at each frame and presents in the
            // transaction when it is set.
            Self::Wgpu { target, .. } => {
                target.setPresentsWithTransaction(presents_with_transaction)
            }
            Self::Native(renderer) => {
                renderer.set_presents_with_transaction(presents_with_transaction)
            }
        }
    }

    fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        match self {
            Self::Wgpu { renderer, .. } => renderer.renderer_mut().update_drawable_size(size),
            Self::Native(renderer) => renderer.update_drawable_size(size),
        }
    }

    fn update_transparency(&mut self, transparent: bool) {
        match self {
            // The surface configuration sets the opacity of the layer.
            Self::Wgpu { renderer, .. } => renderer.renderer_mut().update_transparency(transparent),
            Self::Native(renderer) => renderer.update_transparency(transparent),
        }
    }

    fn destroy(&mut self) {
        match self {
            Self::Wgpu { renderer, .. } => renderer.renderer_mut().destroy(),
            Self::Native(renderer) => renderer.destroy(),
        }
    }

    fn draw(&mut self, scene: &Scene) {
        match self {
            Self::Wgpu { renderer, .. } => renderer.draw(scene),
            Self::Native(renderer) => renderer.draw(scene),
        }
    }

    fn render_to_pending_image(&mut self, scene: &Scene) -> Result<gpui::PendingImage> {
        match self {
            Self::Wgpu { renderer, .. } => renderer.render_to_pending_image(scene),
            Self::Native(renderer) => renderer.render_to_pending_image(scene),
        }
    }
}
