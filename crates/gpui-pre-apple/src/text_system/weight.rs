//! The weights that a face draws. A variable face draws each requested
//! weight, clamped to its weight axis, and gets one `FontId` for each weight
//! that it draws. A named instance of a variable font, such as a face of the
//! system font, and a static face match at their own weight.

use super::{AppleTextSystemState, fontkit_style, fontkit_weight};
use crate::open_type::{WEIGHT_AXIS, objc2_font, with_weight};
use core_text::font::CTFont;
use font_kit::{
    font::Font as FontKitFont,
    properties::{Properties, Weight as FontkitWeight},
};
use gpui::{Font, FontId, FontWeight, Result};
use objc2_core_foundation::{CFDictionary, CFNumber, CFString, CFType};
use objc2_core_text::{
    kCTFontVariationAxisDefaultValueKey, kCTFontVariationAxisIdentifierKey,
    kCTFontVariationAxisMaximumValueKey, kCTFontVariationAxisMinimumValueKey,
};
use smallvec::SmallVec;

struct WeightAxis {
    min: f32,
    max: f32,
    default: f32,
}

impl WeightAxis {
    /// The weight of the axis nearest to `weight`, in whole units.
    fn clamp(&self, weight: f32) -> u16 {
        // `f32::clamp` panics on a malformed axis with a minimum above its
        // maximum.
        weight.max(self.min).min(self.max).round() as u16
    }
}

/// The weight axis of the font, or `None` for a font with no weight axis.
fn weight_axis(font: &CTFont) -> Option<WeightAxis> {
    // SAFETY: Core Text gives an array of variation axis dictionaries, with
    // `CFString` keys.
    let axes = unsafe { objc2_font(font).variation_axes() }?;
    let axes = unsafe { axes.cast_unchecked::<CFDictionary<CFString, CFType>>() };
    // SAFETY: The keys are constants of Core Text.
    let (identifier, min, max, default) = unsafe {
        (
            kCTFontVariationAxisIdentifierKey,
            kCTFontVariationAxisMinimumValueKey,
            kCTFontVariationAxisMaximumValueKey,
            kCTFontVariationAxisDefaultValueKey,
        )
    };
    axes.iter().find_map(|axis| {
        let number = |key: &CFString| axis.get(key)?.downcast::<CFNumber>().ok()?.as_f64();
        if number(identifier)? as i64 != WEIGHT_AXIS {
            return None;
        }
        Some(WeightAxis {
            min: number(min)? as f32,
            max: number(max)? as f32,
            default: number(default)? as f32,
        })
    })
}

/// The value that the font sets on its weight axis. `None` for a font at the
/// default of its axis, or with no weight axis.
pub(super) fn weight_variation(font: &CTFont) -> Option<f32> {
    // SAFETY: Core Text gives a dictionary of number values with variation
    // identifier number keys.
    let variation = unsafe { objc2_font(font).variation() }?;
    let variation = unsafe { variation.cast_unchecked::<CFNumber, CFNumber>() };
    let weight = variation.get(&CFNumber::new_i64(WEIGHT_AXIS))?.as_f64()?;
    Some(weight as f32)
}

/// The weight that a variable face draws with no copy.
fn own_weight(font: &CTFont, axis: &WeightAxis) -> u16 {
    weight_variation(font).unwrap_or(axis.default).round() as u16
}

/// The weight that a face draws with no copy. `None` for a face with no
/// weight axis.
pub(super) fn drawn_weight(font: &CTFont) -> Option<u16> {
    Some(own_weight(font, &weight_axis(font)?))
}

/// The weight of the copy of `font` that draws a request. `None` when the
/// font draws the request itself: a static font, or a variable font that
/// draws the weight of its axis nearest to the request.
fn copy_weight(font: &CTFont, requested: FontWeight) -> Option<u16> {
    let axis = weight_axis(font)?;
    let weight = axis.clamp(requested.0);
    (own_weight(font, &axis) != weight).then_some(weight)
}

/// The properties that a face matches a request of `requested` with. A
/// variable face at the default of its axis offers each weight of its axis.
/// A named instance offers the weight that it sets.
fn match_properties(font: &FontKitFont, requested: FontWeight) -> Properties {
    let mut properties = font.properties();
    let native_font = font.native_font();
    if let Some(axis) = weight_axis(&native_font) {
        let weight = match weight_variation(&native_font) {
            Some(_) => own_weight(&native_font, &axis),
            None => axis.clamp(requested.0),
        };
        properties.weight = FontkitWeight(f32::from(weight));
    }
    properties
}

impl AppleTextSystemState {
    /// The font that draws `font` with the best match among the faces
    /// `candidates` of its family.
    pub(super) fn best_match(&mut self, candidates: &[FontId], font: &Font) -> Result<FontId> {
        let properties = candidates
            .iter()
            .map(|font_id| match_properties(&self.fonts[font_id.0], font.weight))
            .collect::<SmallVec<[_; 4]>>();
        let ix = font_kit::matching::find_best_match(
            &properties,
            &Properties {
                style: fontkit_style(font.style),
                weight: fontkit_weight(font.weight),
                stretch: Default::default(),
            },
        )?;
        // Of the faces that match as well, take one that draws the weight
        // with no copy.
        let face = candidates
            .iter()
            .zip(&properties)
            .filter(|(_, candidate)| **candidate == properties[ix])
            .map(|(&font_id, _)| font_id)
            .find(|font_id| {
                copy_weight(&self.fonts[font_id.0].native_font(), font.weight).is_none()
            })
            .unwrap_or(candidates[ix]);
        Ok(self.font_at_weight(face, font.weight))
    }

    /// The font that draws `face` at the weight nearest to `requested`. Each
    /// cache keyed by `FontId` thus covers the weight.
    fn font_at_weight(&mut self, face: FontId, requested: FontWeight) -> FontId {
        let Some(weight) = copy_weight(&self.fonts[face.0].native_font(), requested) else {
            return face;
        };
        let key = (face, weight);
        if let Some(&font_id) = self.font_ids_by_weight.get(&key) {
            return font_id;
        }
        let font_id = self.push_font(with_weight(&self.fonts[face.0], weight));
        self.font_ids_by_weight.insert(key, font_id);
        font_id
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use crate::AppleTextSystem;
    use anyhow::{Context as _, Result, bail};
    use core_text::font::kCTFontSystemFontType;
    use gpui::{
        Bounds, DevicePixels, Font, FontFallbacks, FontRun, FontWeight, PlatformTextSystem as _,
        font, px,
        variable_font_conformance::{
            self, STATIC_FAMILY, VARIABLE_FAMILY, glyph_params, ink, weighted,
        },
    };
    use std::{borrow::Cow, sync::Arc};

    #[test]
    fn a_variable_face_draws_each_requested_weight() -> Result<()> {
        variable_font_conformance::draws_each_requested_weight(&AppleTextSystem::new())
    }

    #[test]
    fn a_variable_face_draws_the_nearest_weight_of_its_axis() -> Result<()> {
        variable_font_conformance::draws_the_nearest_weight_of_the_axis(&AppleTextSystem::new())
    }

    #[test]
    fn a_face_draws_the_nearest_whole_weight() -> Result<()> {
        variable_font_conformance::draws_the_nearest_whole_weight(Arc::new(AppleTextSystem::new()))
    }

    #[test]
    fn a_variable_face_draws_the_default_optical_size_at_each_size() -> Result<()> {
        variable_font_conformance::draws_the_default_optical_size(&AppleTextSystem::new())
    }

    /// Core Text picks a system face for a glyph that a variable face has
    /// not. The system face has the weight of the variable face. The
    /// fallbacks of a font find no added font on macOS, so the test uses none.
    #[test]
    fn a_fallback_face_draws_at_the_weight_of_its_run() -> Result<()> {
        let fonts = AppleTextSystem::new();
        variable_font_conformance::add_fonts(&fonts)?;
        let inks = [400.0, 700.0]
            .into_iter()
            .map(|weight| {
                let font_id = fonts.font_id(&weighted(VARIABLE_FAMILY, weight))?;
                let text = "m\u{0436}";
                let runs = [FontRun {
                    len: text.len(),
                    font_id,
                }];
                let layout = fonts.layout_line(text, px(16.0), &runs);
                let [run, fallback] = layout.runs.as_slice() else {
                    bail!("expected two runs, got {:?}", layout.runs);
                };
                assert_eq!(run.font_id, font_id);
                ink(&fonts, fallback.font_id, '\u{0436}')
            })
            .collect::<Result<Vec<_>>>()?;
        assert!(inks[0] < inks[1], "{inks:?}");
        Ok(())
    }

    #[test]
    fn many_weights_in_many_sizes_load_one_font_per_drawn_weight() -> Result<()> {
        let fonts = AppleTextSystem::new();
        variable_font_conformance::loads_one_font_per_drawn_weight(&fonts)?;
        assert_eq!(fonts.0.read().fonts.len(), 801);
        Ok(())
    }

    fn system_font(fonts: &AppleTextSystem, weight: f32) -> Result<gpui::FontId> {
        fonts.font_id(&gpui::Font {
            weight: FontWeight(weight),
            ..font(".SystemUIFont")
        })
    }

    /// The faces of the system font are named instances of one variable font.
    #[test]
    fn the_system_font_draws_each_requested_weight() -> Result<()> {
        let fonts = AppleTextSystem::new();
        let inks = [300.0, 400.0, 500.0, 550.0, 600.0, 700.0, 800.0]
            .into_iter()
            .map(|weight| ink(&fonts, system_font(&fonts, weight)?, 'm'))
            .collect::<Result<Vec<_>>>()?;
        assert!(inks.is_sorted_by(|a, b| a < b), "{inks:?}");
        Ok(())
    }

    /// A request of the weight of a named instance of the system font draws
    /// that instance, with no copy.
    #[test]
    fn the_system_font_draws_a_named_weight_with_its_own_face() -> Result<()> {
        let fonts = AppleTextSystem::new();
        let bold = system_font(&fonts, 700.0)?;
        let state = fonts.0.read();
        assert!(
            state
                .font_ids_by_font_key
                .values()
                .any(|faces| faces.contains(&bold))
        );
        assert_eq!(
            super::drawn_weight(&state.fonts[bold.0].native_font()),
            Some(700)
        );
        Ok(())
    }

    /// Core Text sets the optical size of the system font from the font size,
    /// as in each other app of the platform. Its text gets narrower relative
    /// to its size as the size grows.
    #[test]
    fn the_system_font_draws_the_optical_size_of_each_size() -> Result<()> {
        let fonts = AppleTextSystem::new();
        let font_id = system_font(&fonts, 400.0)?;
        let text = "mmmmiiii";
        let runs = [FontRun {
            len: text.len(),
            font_id,
        }];
        let widths = [11.0, 17.0, 28.0, 64.0]
            .map(|size| f32::from(fonts.layout_line(text, px(size), &runs).width) / size);
        assert!(widths.is_sorted_by(|a, b| a > b), "{widths:?}");
        Ok(())
    }

    /// The data of the file of the system font.
    fn system_font_file() -> Result<Vec<u8>> {
        let system_font = core_text::font::new_ui_font_for_language(kCTFontSystemFontType, 0.0, None);
        let path = system_font
            .url()
            .and_then(|url| url.to_path())
            .context("the system font has no file")?;
        std::fs::read(&path).with_context(|| format!("cannot read {}", path.display()))
    }

    /// How a test loads the system font before it lays out a fallback to it.
    #[derive(Clone, Copy, Debug)]
    enum SystemFontLoad {
        None,
        System,
        AddedCopy,
    }

    /// The raster bounds of a glyph that a font takes from its fallback to
    /// the system font, at 11 and at 64 pixels.
    fn fallback_rasters(load: SystemFontLoad) -> Result<Vec<Bounds<DevicePixels>>> {
        let fonts = AppleTextSystem::new();
        variable_font_conformance::add_fonts(&fonts)?;
        let bytes = system_font_file()?;
        let family = font_kit::font::Font::from_bytes(Arc::new(bytes.clone()), 0)?.family_name();
        match load {
            SystemFontLoad::None => {}
            SystemFontLoad::System => {
                system_font(&fonts, 400.0)?;
            }
            SystemFontLoad::AddedCopy => {
                fonts.add_fonts(vec![Cow::Owned(bytes)])?;
                fonts.font_id(&font(family.clone()))?;
            }
        }
        let font_id = fonts.font_id(&Font {
            fallbacks: Some(FontFallbacks::from_fonts(vec![family])),
            ..font(STATIC_FAMILY)
        })?;
        let text = "m\u{1c4}";
        let runs = [FontRun {
            len: text.len(),
            font_id,
        }];
        [11.0, 64.0]
            .into_iter()
            .map(|size| {
                let layout = fonts.layout_line(text, px(size), &runs);
                let [run, fallback] = layout.runs.as_slice() else {
                    bail!("{load:?}: expected two runs, got {:?}", layout.runs);
                };
                assert_eq!(run.font_id, font_id, "{load:?}");
                let params = glyph_params(&fonts, fallback.font_id, '\u{1c4}', size)?;
                fonts.glyph_raster_bounds(&params)
            })
            .collect()
    }

    /// A face that Core Text picks from a cascade list draws the optical size
    /// of its font size. It draws the same when the system font or an added
    /// copy of it loaded before.
    #[test]
    fn a_cascade_face_draws_the_same_after_a_load_of_its_font() -> Result<()> {
        let alone = fallback_rasters(SystemFontLoad::None)?;
        assert_eq!(fallback_rasters(SystemFontLoad::System)?, alone);
        assert_eq!(fallback_rasters(SystemFontLoad::AddedCopy)?, alone);
        Ok(())
    }
}
