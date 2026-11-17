//! The weights that a face draws. A variable face draws each requested
//! weight, clamped to its weight axis, and gets one `FontId` for each weight
//! that it draws. A static face draws only its own weight.

use super::{
    CosmicTextSystemState, FontKey, LoadedFont, LoadedFontKey, check_is_known_emoji_font,
    find_best_match, gpui_style,
};
use anyhow::{Context as _, Result};
use cosmic_text::{FontFeatures as CosmicFontFeatures, Weight, fontdb};
use gpui::FontId;
use itertools::Itertools as _;
use smallvec::SmallVec;
use std::{ops::RangeInclusive, sync::Arc};
use swash::{FontRef, Tag, tag_from_bytes};

const WEIGHT_AXIS: Tag = tag_from_bytes(b"wght");

pub(super) fn cosmic_weight(weight: gpui::FontWeight) -> Weight {
    Weight(weight.0 as u16)
}

fn weight_axis(font: FontRef<'_>) -> Option<RangeInclusive<f32>> {
    let axis = font.variations().find_by_tag(WEIGHT_AXIS)?;
    Some(axis.min_value()..=axis.max_value())
}

/// The weight that a face draws for a request. A variable face draws the
/// request clamped to `axis`. A static face draws its own weight `own`.
fn drawn_weight(axis: Option<&RangeInclusive<f32>>, own: Weight, requested: Weight) -> Weight {
    match axis {
        Some(axis) => {
            // `f32::clamp` panics on a malformed axis with a minimum above
            // its maximum.
            let weight = f32::from(requested.0).max(*axis.start()).min(*axis.end());
            Weight(weight.round() as u16)
        }
        None => own,
    }
}

impl LoadedFont {
    pub(super) fn weight_for(&self, requested: Weight) -> Weight {
        drawn_weight(self.weight_axis.as_ref(), self.weight, requested)
    }
}

impl CosmicTextSystemState {
    /// Loads face `id` at the weight that it draws for `requested`, with no
    /// user fallbacks. The font is the instance of cosmic-text at that
    /// weight, so that shaping and drawing use the same variation.
    pub(super) fn load_face(
        &mut self,
        id: fontdb::ID,
        requested: Weight,
        features: CosmicFontFeatures,
    ) -> Result<LoadedFont> {
        let db = self.font_system.db();
        let face = db
            .face(id)
            .context("the font face is not in the database")?;
        let own = face.weight;
        let is_known_emoji_font = check_is_known_emoji_font(&face.post_script_name);
        // A face with no family name takes its PostScript name.
        let family_name = face
            .families
            .first()
            .map_or(&face.post_script_name, |(name, _)| name)
            .clone()
            .into();
        let (stretch, style, monospaced) = (face.stretch, face.style, face.monospaced);
        let weight_axis = db
            .with_face_data(id, |data, index| {
                weight_axis(FontRef::from_index(data, index as usize)?)
            })
            .context("could not read the font")?;
        let weight = drawn_weight(weight_axis.as_ref(), own, requested);
        let font = self
            .font_system
            .get_font(id, weight)
            .context("could not load the font")?;
        let coords = if weight_axis.is_some() {
            font.as_swash()
                .variations()
                .normalized_coords([(WEIGHT_AXIS, f32::from(weight.0))])
                .collect()
        } else {
            SmallVec::new()
        };
        Ok(LoadedFont {
            font,
            family_name,
            stretch,
            style,
            monospaced,
            weight,
            weight_axis,
            coords,
            features,
            is_known_emoji_font,
            user_fallback_chain: Arc::from([]),
        })
    }

    pub(super) fn push_loaded_font(&mut self, loaded_font: LoadedFont) -> FontId {
        let font_id = FontId(self.loaded_fonts.len());
        self.font_ids_by_shaped_face
            .entry((loaded_font.font.id(), loaded_font.weight))
            .or_insert(font_id);
        self.loaded_fonts.push(loaded_font);
        font_id
    }

    /// The font that draws `face` of the family of `font_key` at the weight
    /// closest to `requested`. Each cache keyed by `FontId` thus covers the
    /// weight.
    pub(super) fn font_at_weight(
        &mut self,
        font_key: &FontKey,
        face: FontId,
        requested: Weight,
    ) -> Result<FontId> {
        let loaded_font = self.loaded_font(face);
        let weight = loaded_font.weight_for(requested);
        if weight == loaded_font.weight {
            return Ok(face);
        }
        let key = LoadedFontKey {
            database_id: loaded_font.font.id(),
            weight,
            font: font_key.clone(),
        };
        if let Some(&font_id) = self.loaded_font_ids_by_key.get(&key) {
            return Ok(font_id);
        }
        let features = loaded_font.features.clone();
        let font = self.load_face(key.database_id, weight, features)?;
        let font_id = self.push_loaded_font(font);
        self.loaded_font_ids_by_key.insert(key, font_id);
        self.loaded_fonts[font_id.0].user_fallback_chain =
            self.user_fallback_chain(font_key, font_id)?;
        Ok(font_id)
    }

    /// Resolves the user fallbacks of each font of `font_key` against the
    /// fonts of the database.
    pub(super) fn resolve_user_fallback_chains(&mut self, font_key: &FontKey) -> Result<()> {
        if font_key.fallbacks.is_none() {
            return Ok(());
        }
        let font_ids = self
            .loaded_font_ids_by_key
            .iter()
            .filter(|(key, _)| key.font == *font_key)
            .map(|(_, &font_id)| font_id)
            .collect_vec();
        for font_id in font_ids {
            self.loaded_fonts[font_id.0].user_fallback_chain =
                self.user_fallback_chain(font_key, font_id)?;
        }
        Ok(())
    }

    /// The face of each fallback family of `font_key` that matches the weight
    /// and the style of font `font_id` best, at that weight. cosmic-text
    /// matches a family only at the weight of the face that it shapes with,
    /// so a static family that lacks the weight of `font_id` must get the
    /// weight of its own face. A missing family is left out, so that a typo
    /// in the settings still lets the font load.
    fn user_fallback_chain(
        &mut self,
        font_key: &FontKey,
        font_id: FontId,
    ) -> Result<Arc<[FontId]>> {
        let Some(fallbacks) = &font_key.fallbacks else {
            return Ok(Arc::from([]));
        };
        let loaded_font = self.loaded_font(font_id);
        let weight = loaded_font.weight;
        let style = gpui_style(loaded_font.style);
        let mut chain = Vec::new();
        for family in fallbacks.fallback_list() {
            // A fallback family has no fallbacks of its own.
            let key = FontKey::new(family.clone().into(), font_key.features.clone(), None);
            let candidates = self.family_font_ids(&key)?;
            if candidates.is_empty() {
                continue;
            }
            let ix = find_best_match(weight, style, &candidates, self)?;
            chain.push(self.font_at_weight(&key, candidates[ix], weight)?);
        }
        Ok(Arc::from(chain))
    }

    /// The font that draws a glyph that cosmic-text shaped with face `id` at
    /// `weight`. This happens when cosmic-text picks a fallback font for some
    /// unicode characters, or a weight that differs from the one of the
    /// requested `FontId`. A face that is not loaded yet is loaded here.
    ///
    /// The font has no features, or the features of an earlier load of the
    /// face. It serves only the output of a layout.
    pub(super) fn font_id_for_shaped_face(
        &mut self,
        id: fontdb::ID,
        weight: Weight,
    ) -> Result<FontId> {
        if let Some(&font_id) = self.font_ids_by_shaped_face.get(&(id, weight)) {
            return Ok(font_id);
        }
        let font = self.load_face(id, weight, CosmicFontFeatures::new())?;
        let font_id = match self.font_ids_by_shaped_face.get(&(id, font.weight)) {
            Some(&font_id) => font_id,
            None => self.push_loaded_font(font),
        };
        self.font_ids_by_shaped_face.insert((id, weight), font_id);
        Ok(font_id)
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::super::CosmicTextSystem;
    use anyhow::{Context as _, Result};
    use cosmic_text::{Fallback, FontSystem, fontdb};
    use gpui::{
        Font, FontFallbacks, FontRun, FontStyle, FontWeight, PlatformTextSystem as _, font, px,
        variable_font_conformance::{self, STATIC_FAMILY, VARIABLE_FAMILY, weighted},
    };
    use itertools::Itertools as _;
    use std::{borrow::Cow, path::Path, sync::Arc};
    use unicode_script::Script;

    fn text_system() -> CosmicTextSystem {
        CosmicTextSystem::new_without_system_fonts(VARIABLE_FAMILY)
    }

    #[test]
    fn a_variable_face_draws_each_requested_weight() -> Result<()> {
        variable_font_conformance::draws_each_requested_weight(&text_system())
    }

    #[test]
    fn a_variable_face_draws_the_nearest_weight_of_its_axis() -> Result<()> {
        variable_font_conformance::draws_the_nearest_weight_of_the_axis(&text_system())
    }

    #[test]
    fn a_fallback_face_draws_at_the_weight_of_its_run() -> Result<()> {
        variable_font_conformance::fallback_draws_at_the_weight_of_its_run(&text_system())
    }

    #[test]
    fn a_face_draws_the_nearest_whole_weight() -> Result<()> {
        variable_font_conformance::draws_the_nearest_whole_weight(Arc::new(text_system()))
    }

    #[test]
    fn a_variable_face_draws_the_default_optical_size_at_each_size() -> Result<()> {
        variable_font_conformance::draws_the_default_optical_size(&text_system())
    }

    /// The fonts and the glyph images of many weights in many sizes stay
    /// bounded.
    #[test]
    fn many_weights_in_many_sizes_load_one_font_per_drawn_weight() -> Result<()> {
        let text_system = text_system();
        variable_font_conformance::loads_one_font_per_drawn_weight(&text_system)?;
        let state = text_system.0.read();
        assert_eq!(state.loaded_fonts.len(), 801);
        assert!(state.pending_glyph_images.is_empty());
        Ok(())
    }

    #[test]
    fn each_font_reports_the_weight_that_it_draws() -> Result<()> {
        let text_system = text_system();
        variable_font_conformance::add_fonts(&text_system)?;
        for (requested, drawn) in [
            (50.0, 100.0),
            (550.0, 550.0),
            (700.0, 700.0),
            (950.0, 900.0),
        ] {
            let font_id = text_system.font_id(&weighted(VARIABLE_FAMILY, requested))?;
            assert_eq!(
                text_system.font_weight_and_style(font_id)?,
                (FontWeight(drawn), FontStyle::Normal)
            );
        }
        let static_regular = text_system.font_id(&weighted(STATIC_FAMILY, 500.0))?;
        assert_eq!(
            text_system.font_weight_and_style(static_regular)?,
            (FontWeight::NORMAL, FontStyle::Normal)
        );
        Ok(())
    }

    /// A face that cosmic-text picks from the font database, with no fallback
    /// of the font, draws at the weight of its run.
    #[test]
    fn a_fallback_face_of_the_database_draws_at_the_weight_of_its_run() -> Result<()> {
        let text_system = text_system();
        variable_font_conformance::add_fonts(&text_system)?;
        let variable_bold = text_system.font_id(&weighted(VARIABLE_FAMILY, 700.0))?;
        let static_bold = text_system.font_id(&weighted(STATIC_FAMILY, 700.0))?;
        let text = "m\u{2318}";
        let runs = [FontRun {
            len: text.len(),
            font_id: static_bold,
        }];
        let layout = text_system.layout_line(text, px(16.0), &runs);
        let font_ids = layout.runs.iter().map(|run| run.font_id).collect_vec();
        assert_eq!(font_ids, [static_bold, variable_bold]);
        Ok(())
    }

    /// The font system falls back to `STATIC_FAMILY` for a character that the
    /// families of a span do not have, as it does to the fonts of the system.
    struct StaticFamilyFallback;

    impl Fallback for StaticFamilyFallback {
        fn common_fallback(&self) -> &[&'static str] {
            &[STATIC_FAMILY]
        }

        fn forbidden_fallback(&self) -> &[&'static str] {
            &[]
        }

        fn script_fallback(&self, _script: Script, _locale: &str) -> &[&'static str] {
            &[]
        }
    }

    /// A fallback family with no face at the weight of its run draws its
    /// face nearest to that weight. The font system does not pick its own
    /// fallback for it.
    #[test]
    fn a_fallback_family_with_no_face_at_the_weight_draws_its_nearest_face() -> Result<()> {
        const REGULAR_ONLY_FAMILY: &str = "IBM Plex Sans";
        let font_system = FontSystem::new_with_locale_and_db_and_fallback(
            "en-US".to_owned(),
            fontdb::Database::new(),
            StaticFamilyFallback,
        );
        let text_system = CosmicTextSystem::with_font_system(font_system, VARIABLE_FAMILY);
        variable_font_conformance::add_fonts(&text_system)?;
        text_system.add_fonts(vec![Cow::Borrowed(include_bytes!(
            "../../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf"
        ))])?;
        let regular = text_system.font_id(&font(REGULAR_ONLY_FAMILY))?;
        let bold = text_system.font_id(&Font {
            fallbacks: Some(FontFallbacks::from_fonts(vec![
                REGULAR_ONLY_FAMILY.to_owned(),
            ])),
            ..weighted(VARIABLE_FAMILY, 700.0)
        })?;
        // The variable font has no U+00E9. Both static families have it.
        let text = "m\u{e9}";
        let runs = [FontRun {
            len: text.len(),
            font_id: bold,
        }];
        let layout = text_system.layout_line(text, px(16.0), &runs);
        let font_ids = layout.runs.iter().map(|run| run.font_id).collect_vec();
        assert_eq!(font_ids, [bold, regular]);
        Ok(())
    }

    /// A family that the app adds draws each weight with a face of the app,
    /// also where the system has a copy of the family.
    #[test]
    fn an_added_family_draws_no_face_of_the_system() -> Result<()> {
        let text_system = text_system();
        let file = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/fonts/inter/InterVariable-subset.ttf");
        // The system fonts load before the app adds its fonts.
        text_system
            .0
            .write()
            .font_system
            .db_mut()
            .load_font_file(&file)?;
        text_system.add_fonts(vec![Cow::Owned(std::fs::read(&file)?)])?;
        for weight in [400.0, 500.0, 700.0] {
            let font_id = text_system.font_id(&weighted(VARIABLE_FAMILY, weight))?;
            {
                let state = text_system.0.read();
                let face = state
                    .font_system
                    .db()
                    .face(state.loaded_font(font_id).font.id())
                    .context("the font face is not in the database")?;
                assert!(matches!(face.source, fontdb::Source::Binary(_)), "{weight}");
            }
            let runs = [FontRun { len: 3, font_id }];
            let layout = text_system.layout_line("mmm", px(16.0), &runs);
            let font_ids = layout.runs.iter().map(|run| run.font_id).collect_vec();
            assert_eq!(font_ids, [font_id], "{weight}");
        }
        Ok(())
    }
}
