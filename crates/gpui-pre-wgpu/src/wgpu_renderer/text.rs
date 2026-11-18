//! The settings of glyph blending.

use super::GammaParams;
use gpui::get_gamma_correction_ratios;

/// How text glyphs blend with the background. A platform that has system text
/// settings passes them, so that text looks the same as in its native renderer.
#[derive(Clone, Copy, Debug)]
pub struct TextContrast {
    pub gamma: f32,
    pub grayscale_enhanced_contrast: f32,
    pub subpixel_enhanced_contrast: f32,
}

/// How glyphs blend with the background. It stays the same across devices.
#[derive(Clone, Copy)]
pub(super) struct TextRendering {
    gamma_ratios: [f32; 4],
    grayscale_enhanced_contrast: f32,
    subpixel_enhanced_contrast: f32,
    pub(super) is_bgr: bool,
}

impl TextRendering {
    pub(super) fn from_env() -> Self {
        use std::env;

        let gamma = env::var("ZED_FONTS_GAMMA")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.8_f32)
            .clamp(1.0, 2.2);

        let grayscale_enhanced_contrast = env::var("ZED_FONTS_GRAYSCALE_ENHANCED_CONTRAST")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0_f32)
            .max(0.0);

        let subpixel_enhanced_contrast = env::var("ZED_FONTS_SUBPIXEL_ENHANCED_CONTRAST")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.5_f32)
            .max(0.0);

        Self {
            gamma_ratios: get_gamma_correction_ratios(gamma),
            grayscale_enhanced_contrast,
            subpixel_enhanced_contrast,
            is_bgr: false,
        }
    }

    pub(super) fn set_contrast(&mut self, contrast: TextContrast) {
        self.gamma_ratios = get_gamma_correction_ratios(contrast.gamma);
        self.grayscale_enhanced_contrast = contrast.grayscale_enhanced_contrast;
        self.subpixel_enhanced_contrast = contrast.subpixel_enhanced_contrast;
    }

    pub(super) fn gamma_params(&self) -> GammaParams {
        GammaParams {
            gamma_ratios: self.gamma_ratios,
            grayscale_enhanced_contrast: self.grayscale_enhanced_contrast,
            subpixel_enhanced_contrast: self.subpixel_enhanced_contrast,
            is_bgr: self.is_bgr as u32,
            _pad: 0,
        }
    }
}
