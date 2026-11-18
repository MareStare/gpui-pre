//! The GPU objects of a window renderer and the configuration of its surface.

use super::{WgpuRenderer, WgpuRendererCore, WgpuSurfaceConfig};
use crate::startup_log::time_startup_stage;
use crate::{WgpuAtlas, WgpuContext};
use gpui::{DevicePixels, Size};
use log::warn;
use std::sync::Arc;

/// The composite alpha modes of a surface that the renderer picks from.
#[derive(Clone, Copy)]
pub(crate) struct AlphaModes {
    pub(super) transparent: wgpu::CompositeAlphaMode,
    pub(super) opaque: wgpu::CompositeAlphaMode,
}

impl AlphaModes {
    pub(super) fn pick(self, transparent: bool) -> wgpu::CompositeAlphaMode {
        if transparent {
            self.transparent
        } else {
            self.opaque
        }
    }
}

/// The GPU objects of a windowed renderer, made for one device.
pub(super) struct WindowGpu {
    pub(super) core: WgpuRendererCore,
    pub(super) surface_config: wgpu::SurfaceConfiguration,
    alpha_modes: AlphaModes,
}

impl WindowGpu {
    pub(super) fn new(
        context: &WgpuContext,
        surface: &wgpu::Surface<'static>,
        config: &WgpuSurfaceConfig,
        color_space: wgpu::SurfaceColorSpace,
        surface_configured: bool,
        atlas: &Arc<WgpuAtlas>,
    ) -> anyhow::Result<Self> {
        let (surface_config, alpha_modes) = WgpuRenderer::surface_configuration(
            surface,
            &context.adapter,
            &context.device,
            config,
            color_space,
        )?;
        // The renderer reports the errors of the device that it sees after it starts, so
        // it starts before the surface is configured.
        let core = time_startup_stage("wgpu-renderer-core", || {
            WgpuRendererCore::new(
                context,
                Arc::clone(atlas),
                surface_config.format,
                surface_config.alpha_mode,
            )
        });
        // Adapter selection configures the surface with this same configuration when it
        // creates the context, except for the default color space that it always uses.
        // Configure it here in the other cases.
        if !surface_configured || color_space != wgpu::SurfaceColorSpace::Auto {
            time_startup_stage("wgpu-configure", || {
                surface.configure(&context.device, &surface_config);
            });
        }
        Ok(Self {
            core,
            surface_config,
            alpha_modes,
        })
    }

    /// Sets the size of the surface configuration for `size`, and drops the textures of
    /// the old size. Returns whether it changed.
    pub(super) fn set_size(&mut self, size: Size<DevicePixels>) -> bool {
        let (width, height) = surface_extent(size, self.core.max_texture_size);
        if (width, height) == (self.surface_config.width, self.surface_config.height) {
            return false;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;

        #[cfg(not(target_family = "wasm"))]
        {
            self.core.offscreen_target = None;
        }
        let resources = &mut self.core.resources;

        // Destroy old textures before allocating new ones to avoid GPU memory spikes.
        // wgpu frees a texture that GPU work in flight uses after that work ends.
        if let Some(ref texture) = resources.path_intermediate_texture {
            texture.destroy();
        }
        if let Some(ref texture) = resources.path_msaa_texture {
            texture.destroy();
        }

        // Invalidate intermediate textures - they will be lazily recreated
        // in draw() after we confirm the surface is healthy. This avoids
        // panics when the device/surface is in an invalid state during resize.
        resources.invalidate_intermediate_textures();
        true
    }

    /// Sets the alpha mode of the surface configuration, and rebuilds the pipelines when
    /// it changes. Returns whether it changed.
    pub(super) fn set_alpha_mode(&mut self, transparent: bool) -> bool {
        let alpha_mode = self.alpha_modes.pick(transparent);
        if alpha_mode == self.surface_config.alpha_mode {
            return false;
        }
        self.surface_config.alpha_mode = alpha_mode;
        self.core
            .rebuild_pipelines(self.surface_config.format, alpha_mode);
        true
    }
}

pub(super) fn surface_size(config: &wgpu::SurfaceConfiguration) -> Size<DevicePixels> {
    Size {
        width: DevicePixels(config.width as i32),
        height: DevicePixels(config.height as i32),
    }
}

/// Returns the size of a surface for `size`, which the device can hold and which is not
/// empty.
pub(super) fn surface_extent(size: Size<DevicePixels>, max_texture_size: u32) -> (u32, u32) {
    let requested = (size.width.0.max(0) as u32, size.height.0.max(0) as u32);
    let clamped = (
        requested.0.min(max_texture_size),
        requested.1.min(max_texture_size),
    );
    if clamped != requested {
        warn!(
            "Requested surface size {requested:?} exceeds maximum texture dimension \
             {max_texture_size}. Clamping to {clamped:?}. Window content may not fill the \
             entire window."
        );
    }
    (clamped.0.max(1), clamped.1.max(1))
}
