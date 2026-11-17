//! Finds fonts by a family name, a weight and a style.

use anyhow::{Context as _, Result};
use gpui::{FontStyle, FontWeight};
use gpui_util::ResultExt as _;
use smallvec::{SmallVec, smallvec};
use windows::{
    Win32::Graphics::DirectWrite::*,
    core::{HSTRING, Interface as _, PCWSTR, w},
};

use super::{DirectWriteComponents, get_name};

pub(super) type FontAxisValues = SmallVec<[DWRITE_FONT_AXIS_VALUE; 4]>;

/// The lowest weight that counts as bold.
const BOLD_WEIGHT: f32 = DWRITE_FONT_WEIGHT_SEMI_BOLD.0 as f32;

/// The axes that each simulation changes.
const SIMULATED_AXES: [(DWRITE_FONT_SIMULATIONS, DWRITE_FONT_AXIS_TAG); 3] = [
    (DWRITE_FONT_SIMULATIONS_BOLD, DWRITE_FONT_AXIS_TAG_WEIGHT),
    (DWRITE_FONT_SIMULATIONS_OBLIQUE, DWRITE_FONT_AXIS_TAG_ITALIC),
    (DWRITE_FONT_SIMULATIONS_OBLIQUE, DWRITE_FONT_AXIS_TAG_SLANT),
];

/// Reads an array that DirectWrite fills, of a length that it gave before.
pub(super) fn read_array<T: Copy + Default>(
    len: u32,
    read: impl FnOnce(&mut [T]) -> windows::core::Result<()>,
) -> Result<SmallVec<[T; 4]>> {
    let mut values = smallvec![T::default(); len as usize];
    read(&mut values)?;
    Ok(values)
}

/// A font face and what a layout needs to draw it.
#[derive(Debug)]
pub(super) struct FontMatch {
    pub(super) face: IDWriteFontFace3,
    /// The axis values of the face itself. They do not show its simulations.
    face_values: FontAxisValues,
    /// The axis values that a layout draws the face with. DirectWrite makes
    /// the simulations from the values of the simulated axes.
    pub(super) axis_values: FontAxisValues,
    /// The axes that the font can change.
    variable_axes: SmallVec<[DWRITE_FONT_AXIS_TAG; 4]>,
    pub(super) family: HSTRING,
    file: IDWriteFontFile,
    face_index: u32,
}

impl FontMatch {
    /// The match of a face that a lookup or a layout with the requested axis
    /// values found.
    pub(super) fn new(
        face: IDWriteFontFace3,
        requested: &FontAxisValues,
        components: &DirectWriteComponents,
    ) -> Result<Self> {
        let face_values = face_axis_values(&face)?;
        Self::with_values(face, face_values, requested, components)
    }

    fn with_values(
        face: IDWriteFontFace3,
        face_values: FontAxisValues,
        requested: &FontAxisValues,
        components: &DirectWriteComponents,
    ) -> Result<Self> {
        let simulations = unsafe { face.GetSimulations() };
        let mut axis_values = face_values.clone();
        for (_, tag) in SIMULATED_AXES
            .into_iter()
            .filter(|&(simulation, _)| simulations.contains(simulation))
        {
            let Some(requested) = requested.iter().find(|value| value.axisTag == tag) else {
                continue;
            };
            match axis_values.iter_mut().find(|value| value.axisTag == tag) {
                Some(value) => value.value = requested.value,
                None => axis_values.push(*requested),
            }
        }
        let family = HSTRING::from(get_name(
            unsafe { face.GetFamilyNames()? },
            &components.locale,
        )?);
        let resource = unsafe { face.cast::<IDWriteFontFace5>()?.GetFontResource()? };
        let ranges = read_array::<DWRITE_FONT_AXIS_RANGE>(
            unsafe { resource.GetFontAxisCount() },
            |ranges| unsafe { resource.GetFontAxisRanges(ranges) },
        )?;
        let variable_axes = ranges
            .iter()
            .filter(|range| range.minValue < range.maxValue)
            .map(|range| range.axisTag)
            .collect();
        let file = unsafe { resource.GetFontFile()? };
        let face_index = unsafe { resource.GetFontFaceIndex() };
        Ok(Self {
            face,
            face_values,
            axis_values,
            variable_axes,
            family,
            file,
            face_index,
        })
    }

    /// A collection with the face of this match and no other, in the family
    /// `family`. A layout with that family draws the face at any axis values.
    pub(super) fn collection(
        &self,
        factory: &IDWriteFactory6,
        family: &HSTRING,
    ) -> Result<IDWriteFontCollection2> {
        one_face_collection(
            factory,
            &self.file,
            self.face_index,
            family,
            &self.face_values,
        )
    }

    /// A collection with the face of this match and no other, in the family
    /// of the match, for a fallback of a run with the axis values
    /// `run_values`. DirectWrite makes the simulations of a run from the axis
    /// values of the run and the axis values that the collection gives for
    /// the face. Thus the face gets the value of the run on each axis that a
    /// simulation changes, if the font has one value on that axis and the
    /// match has no such simulation. The run then gets only the simulations
    /// of this match.
    pub(super) fn fallback_collection(
        &self,
        factory: &IDWriteFactory6,
        run_values: &FontAxisValues,
    ) -> Result<IDWriteFontCollection2> {
        let simulations = unsafe { self.face.GetSimulations() };
        let takes_run_value = |tag| {
            !self.variable_axes.contains(&tag)
                && SIMULATED_AXES
                    .iter()
                    .any(|&(simulation, axis)| axis == tag && !simulations.contains(simulation))
        };
        let mut values = self.face_values.clone();
        for value in &mut values {
            if takes_run_value(value.axisTag)
                && let Some(run_value) = run_values
                    .iter()
                    .find(|run_value| run_value.axisTag == value.axisTag)
            {
                value.value = run_value.value;
            }
        }
        one_face_collection(factory, &self.file, self.face_index, &self.family, &values)
    }
}

/// A collection with one face of a font file, in the family `family` and at
/// the axis values `values`.
fn one_face_collection(
    factory: &IDWriteFactory6,
    file: &IDWriteFontFile,
    face_index: u32,
    family: &HSTRING,
    values: &FontAxisValues,
) -> Result<IDWriteFontCollection2> {
    // An axis range with one value keeps a variable font at that value.
    let ranges: SmallVec<[DWRITE_FONT_AXIS_RANGE; 4]> = values
        .iter()
        .map(|value| DWRITE_FONT_AXIS_RANGE {
            axisTag: value.axisTag,
            minValue: value.value,
            maxValue: value.value,
        })
        .collect();
    // The font set takes only these properties, and no property of the font
    // file.
    let properties = [
        DWRITE_FONT_PROPERTY_ID_TYPOGRAPHIC_FAMILY_NAME,
        DWRITE_FONT_PROPERTY_ID_WEIGHT_STRETCH_STYLE_FAMILY_NAME,
    ]
    .map(|id| DWRITE_FONT_PROPERTY {
        propertyId: id,
        propertyValue: PCWSTR(family.as_ptr()),
        localeName: w!(""),
    });
    let font_set = unsafe {
        let builder = factory.CreateFontSetBuilder()?;
        builder.AddFont(
            file,
            face_index,
            DWRITE_FONT_SIMULATIONS_NONE,
            values,
            &ranges,
            &properties,
        )?;
        builder.CreateFontSet()?
    };
    font_collection(factory, &font_set)
}

fn face_axis_values(face: &IDWriteFontFace3) -> Result<FontAxisValues> {
    let face: IDWriteFontFace5 = face.cast()?;
    read_array(unsafe { face.GetFontAxisValueCount() }, |values| unsafe {
        face.GetFontAxisValues(values)
    })
}

/// Groups the fonts into families by the typographic family name. A variable
/// font with an axis other than weight, width, italic and slant has one
/// typographic family, and a weight-stretch-style family for each value of
/// that axis. A lookup in this collection finds a family by either name, or
/// by the family name that GDI uses:
/// https://learn.microsoft.com/en-us/windows/win32/api/dwrite_3/ne-dwrite_3-dwrite_font_family_model
pub(super) fn font_collection(
    factory: &IDWriteFactory6,
    font_set: &IDWriteFontSet,
) -> Result<IDWriteFontCollection2> {
    Ok(unsafe {
        factory.CreateFontCollectionFromFontSet(font_set, DWRITE_FONT_FAMILY_MODEL_TYPOGRAPHIC)?
    })
}

/// The system fonts as of the last check for updates. DirectWrite keeps this
/// collection.
pub(super) fn system_font_collection(factory: &IDWriteFactory6) -> Result<IDWriteFontCollection2> {
    Ok(unsafe { factory.GetSystemFontCollection(false, DWRITE_FONT_FAMILY_MODEL_TYPOGRAPHIC)? })
}

/// The system fonts after a check for updates. The check finds the fonts that
/// were installed after the last check, but it takes tens of milliseconds:
/// https://learn.microsoft.com/en-us/windows/win32/api/dwrite_3/nf-dwrite_3-idwritefactory3-getsystemfontcollection
pub(super) fn updated_system_font_collection(
    factory: &IDWriteFactory6,
) -> Result<IDWriteFontCollection2> {
    let font_set = unsafe {
        let mut collection = None;
        IDWriteFactory3::GetSystemFontCollection(factory, false, &mut collection, true)?;
        collection
            .context("Failed to get system font collection")?
            .GetFontSet()?
    };
    font_collection(factory, &font_set)
}

/// The axis values of a weight and a style. The other axes keep the values of
/// the family, such as the width of a condensed family. Maps a style to the
/// italic and slant axes as WinUI does:
/// https://github.com/microsoft/microsoft-ui-xaml/blob/8463f45162149de0ec3ad7df752596893fe3e13e/dxaml/xcp/pal/win/inc/DWriteFontAndScriptServices.h#L55-L71
fn font_axis_values(weight: FontWeight, style: FontStyle) -> FontAxisValues {
    let (italic, slant) = match style {
        FontStyle::Normal => (0.0, 0.0),
        FontStyle::Italic => (1.0, -12.0),
        FontStyle::Oblique => (0.0, -20.0),
    };
    [
        (DWRITE_FONT_AXIS_TAG_WEIGHT, weight.0),
        (DWRITE_FONT_AXIS_TAG_ITALIC, italic),
        (DWRITE_FONT_AXIS_TAG_SLANT, slant),
    ]
    .into_iter()
    .map(|(tag, value)| DWRITE_FONT_AXIS_VALUE {
        axisTag: tag,
        value,
    })
    .collect()
}

/// Finds the font of a family nearest to a weight and a style, in the first
/// collection that has the family. A variable font takes the weight within its
/// range, also between its named instances:
/// https://learn.microsoft.com/en-us/windows/win32/api/dwrite_3/nf-dwrite_3-idwritefontcollection2-getmatchingfonts
pub(super) fn find_font(
    collections: &[&IDWriteFontCollection2],
    family: &HSTRING,
    weight: FontWeight,
    style: FontStyle,
    components: &DirectWriteComponents,
) -> Option<FontMatch> {
    let axis_values = font_axis_values(weight, style);
    collections.iter().find_map(|&collection| {
        // A collection without the family gives an empty list.
        let fonts = unsafe { collection.GetMatchingFonts(family, &axis_values) }.log_err()?;
        ranked_candidates(&fonts, weight.0, style)
            .into_iter()
            .find_map(|candidate| candidate.into_match(&axis_values, components).log_err())
    })
}

/// The fonts of the list from the best to the worst: the width nearest to the
/// width of the first font, then the style in the order of CSS, then a bold
/// face for a bold request, then the fewest simulations, then the weight in
/// the order of CSS. The order of DirectWrite breaks a tie. DirectWrite can put
/// a simulation first, such as a regular face made bold before the semibold
/// face, a bold face made bolder, or a regular face made oblique before the
/// italic face. The orders of CSS:
/// https://www.w3.org/TR/css-fonts-4/#font-style-matching
fn ranked_candidates(fonts: &IDWriteFontList2, weight: f32, style: FontStyle) -> Vec<Candidate> {
    let mut candidates: Vec<_> = (0..unsafe { fonts.GetFontCount() })
        .filter_map(|index| {
            let font = unsafe { fonts.GetFont(index) }.log_err()?;
            Candidate::new(&font).log_err()
        })
        .collect();
    // A family name can imply a width, such as the name of a condensed family.
    let Some(width) = candidates.first().map(|candidate| candidate.width) else {
        return candidates;
    };
    let rank = |candidate: &Candidate| {
        (
            (candidate.width - width).abs(),
            candidate.style_rank(style),
            weight >= BOLD_WEIGHT && !candidate.is_bold(),
            candidate.simulations.0.count_ones(),
            weight_rank(candidate.weight, weight),
        )
    };
    // The sort is stable, so the order of DirectWrite breaks a tie.
    candidates.sort_by(|a, b| {
        rank(a)
            .partial_cmp(&rank(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates
}

/// The rank of a weight for a requested weight in the order of CSS. A request
/// from 400 to 500 takes the weights up to 500 first, then the lighter
/// weights, then the heavier ones. A lighter request takes the lighter
/// weights first, a heavier request the heavier weights:
/// https://www.w3.org/TR/css-fonts-4/#font-matching-algorithm
fn weight_rank(weight: f32, requested: f32) -> (u8, f32) {
    let distance = (weight - requested).abs();
    let group = if (400.0..=500.0).contains(&requested) {
        if (requested..=500.0).contains(&weight) {
            0
        } else if weight < requested {
            1
        } else {
            2
        }
    } else if requested < 400.0 {
        u8::from(weight > requested)
    } else {
        u8::from(weight < requested)
    };
    (group, distance)
}

/// A font of a list with only the simulations that it needs.
struct Candidate {
    reference: IDWriteFontFaceReference1,
    face_values: FontAxisValues,
    simulations: DWRITE_FONT_SIMULATIONS,
    /// The weight, the width and the style of the face without simulations.
    weight: f32,
    width: f32,
    style: FontStyle,
}

impl Candidate {
    /// A bold face needs no bold simulation. An italic or oblique face needs no
    /// oblique simulation.
    fn new(font: &IDWriteFont3) -> Result<Self> {
        let reference: IDWriteFontFaceReference1 =
            unsafe { font.GetFontFaceReference()? }.cast()?;
        let face_values = read_array(
            unsafe { reference.GetFontAxisValueCount() },
            |values| unsafe { reference.GetFontAxisValues(values) },
        )?;
        let axis_value = |tag, default| {
            face_values
                .iter()
                .find(|value: &&DWRITE_FONT_AXIS_VALUE| value.axisTag == tag)
                .map_or(default, |value| value.value)
        };
        let weight = axis_value(DWRITE_FONT_AXIS_TAG_WEIGHT, FontWeight::NORMAL.0);
        let width = axis_value(DWRITE_FONT_AXIS_TAG_WIDTH, 100.0);
        let style = if axis_value(DWRITE_FONT_AXIS_TAG_ITALIC, 0.0) > 0.0 {
            FontStyle::Italic
        } else if axis_value(DWRITE_FONT_AXIS_TAG_SLANT, 0.0) < 0.0 {
            FontStyle::Oblique
        } else {
            FontStyle::Normal
        };
        let mut simulations = unsafe { reference.GetSimulations() };
        if weight >= BOLD_WEIGHT {
            simulations &= !DWRITE_FONT_SIMULATIONS_BOLD;
        }
        if style != FontStyle::Normal {
            simulations &= !DWRITE_FONT_SIMULATIONS_OBLIQUE;
        }
        Ok(Self {
            reference,
            face_values,
            simulations,
            weight,
            width,
            style,
        })
    }

    fn into_match(
        self,
        requested: &FontAxisValues,
        components: &DirectWriteComponents,
    ) -> Result<FontMatch> {
        let face = unsafe {
            self.reference
                .CreateFontFaceWithSimulations(self.simulations)?
        };
        FontMatch::with_values(face, self.face_values, requested, components)
    }

    fn is_bold(&self) -> bool {
        self.weight >= BOLD_WEIGHT || self.simulations.contains(DWRITE_FONT_SIMULATIONS_BOLD)
    }

    /// The rank of the style of the face for a requested style, in the order
    /// of CSS. A simulated oblique comes after the slanted faces.
    fn style_rank(&self, requested: FontStyle) -> (u8, bool) {
        if self.simulations.contains(DWRITE_FONT_SIMULATIONS_OBLIQUE) {
            let last_slanted = style_order(requested, FontStyle::Oblique)
                .max(style_order(requested, FontStyle::Italic));
            (last_slanted, true)
        } else {
            (style_order(requested, self.style), false)
        }
    }
}

/// The place of a style in the order of CSS for a requested style.
fn style_order(requested: FontStyle, style: FontStyle) -> u8 {
    use FontStyle::*;
    match (requested, style) {
        (Normal, Normal) | (Italic, Italic) | (Oblique, Oblique) => 0,
        (Normal, Oblique) | (Italic, Oblique) | (Oblique, Italic) => 1,
        (Normal, Italic) | (Italic, Normal) | (Oblique, Normal) => 2,
    }
}

/// Whether two faces have the same font file, face index, simulations and
/// axis values.
pub(super) fn same_face(a: &IDWriteFontFace3, b: &IDWriteFontFace3) -> bool {
    a == b
        || a.cast::<IDWriteFontFace5>()
            .is_ok_and(|a| unsafe { a.Equals(b) }.as_bool())
}

#[cfg(test)]
mod tests {
    use super::super::{DirectWriteTextSystem, get_name};
    use super::{face_axis_values, font_collection};
    use anyhow::{Context as _, Result};
    use gpui::{
        Font, FontFallbacks, FontFeatures, FontId, FontRun, FontStyle, FontWeight,
        PlatformTextSystem, px,
    };
    use std::{borrow::Cow, sync::Arc};
    use windows::{
        Win32::Graphics::DirectWrite::*,
        core::{BOOL, HSTRING, Interface as _, PCWSTR, w},
    };

    const INTER_VARIABLE: &[u8] =
        include_bytes!("../../../../assets/fonts/inter/InterVariable-subset.ttf");
    const IBM_PLEX_SANS: &[u8] =
        include_bytes!("../../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf");

    fn axis_value(
        text_system: &DirectWriteTextSystem,
        font_id: FontId,
        tag: DWRITE_FONT_AXIS_TAG,
    ) -> Result<f32> {
        face_axis_values(text_system.state.read().fonts[font_id.0].face())?
            .iter()
            .find(|value| value.axisTag == tag)
            .map(|value| value.value)
            .with_context(|| format!("The font face has no axis {:?}", tag.0.to_le_bytes()))
    }

    fn simulations(
        text_system: &DirectWriteTextSystem,
        font_id: FontId,
    ) -> DWRITE_FONT_SIMULATIONS {
        unsafe {
            text_system.state.read().fonts[font_id.0]
                .face()
                .GetSimulations()
        }
    }

    fn full_name(text_system: &DirectWriteTextSystem, font_id: FontId) -> Result<String> {
        let state = text_system.state.read();
        let mut names = None;
        let mut exists = BOOL::default();
        unsafe {
            state.fonts[font_id.0].face().GetInformationalStrings(
                DWRITE_INFORMATIONAL_STRING_FULL_NAME,
                &mut names,
                &mut exists,
            )?
        };
        get_name(
            names.context("The font face has no full name")?,
            &HSTRING::from("en-us"),
        )
    }

    /// Lays out one character of the text with each font.
    fn layout_font_ids(
        text_system: &DirectWriteTextSystem,
        text: &str,
        font_ids: &[FontId],
    ) -> Vec<FontId> {
        let runs: Vec<_> = text
            .chars()
            .zip(font_ids)
            .map(|(character, &font_id)| FontRun {
                len: character.len_utf8(),
                font_id,
            })
            .collect();
        let layout = text_system.layout_line(text, px(16.0), &runs);
        layout.runs.iter().map(|run| run.font_id).collect()
    }

    // The optical size axis of Inter 4 Variable has a "Text" value that is not
    // elidable. Its weight-stretch-style family is "Inter Variable Text".
    #[test]
    fn variable_font_matches_typographic_family_and_weight_axis() -> Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        text_system.add_fonts(vec![Cow::Borrowed(INTER_VARIABLE)])?;
        let weights = [400.0, 500.0, 550.0, 600.0, 700.0];
        for family in ["Inter Variable", "Inter Variable Text"] {
            let mut font_ids = Vec::new();
            for weight in weights {
                let font = Font {
                    family: family.into(),
                    weight: FontWeight(weight),
                    ..Default::default()
                };
                let font_id = text_system.font_id(&font)?;
                assert_eq!(
                    axis_value(&text_system, font_id, DWRITE_FONT_AXIS_TAG_WEIGHT)?,
                    weight,
                    "{family}, {weight}"
                );
                font_ids.push(font_id);
            }
            assert_eq!(
                layout_font_ids(&text_system, "Hambu", &font_ids),
                font_ids,
                "{family}"
            );
        }
        let names = text_system.all_font_names();
        assert!(names.iter().any(|name| name == "Inter Variable"));
        assert!(!names.iter().any(|name| name == "Inter Variable Text"));
        Ok(())
    }

    // Bahnschrift is a system font with a width axis. Its weight-stretch-style
    // family "Bahnschrift Condensed" has the width 75.
    #[test]
    fn family_name_keeps_the_axes_it_implies() -> Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        let mut font_ids = Vec::new();
        for weight in [400.0, 700.0] {
            let font_id = text_system.font_id(&Font {
                family: "Bahnschrift Condensed".into(),
                weight: FontWeight(weight),
                ..Default::default()
            })?;
            assert_eq!(
                axis_value(&text_system, font_id, DWRITE_FONT_AXIS_TAG_WIDTH)?,
                75.0
            );
            font_ids.push(font_id);
        }
        assert_eq!(layout_font_ids(&text_system, "ab", &font_ids), font_ids);
        // "Segoe UI Semibold" names the faces of the weight 600.
        let semibold = text_system.font_id(&Font {
            family: "Segoe UI Semibold".into(),
            ..Default::default()
        })?;
        assert_eq!(full_name(&text_system, semibold)?, "Segoe UI Semibold");
        assert_eq!(layout_font_ids(&text_system, "a", &[semibold]), [semibold]);
        Ok(())
    }

    #[test]
    fn fallback_matches_typographic_family_of_added_font() -> Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        text_system.add_fonts(vec![
            Cow::Borrowed(IBM_PLEX_SANS),
            Cow::Borrowed(INTER_VARIABLE),
        ])?;
        let inter = text_system.font_id(&Font {
            family: "Inter Variable".into(),
            ..Default::default()
        })?;
        let plex = text_system.font_id(&Font {
            family: "IBM Plex Sans".into(),
            fallbacks: Some(FontFallbacks::from_fonts(vec!["Inter Variable".into()])),
            ..Default::default()
        })?;
        // IBM Plex Sans has no glyph for U+2318.
        assert_eq!(
            layout_font_ids(&text_system, "a\u{2318}", &[plex, plex]),
            [plex, inter]
        );
        Ok(())
    }

    #[test]
    fn fallback_face_keeps_its_axes() -> Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        text_system.add_fonts(vec![Cow::Borrowed(INTER_VARIABLE)])?;
        let inter = text_system.font_id(&Font {
            family: "Inter Variable".into(),
            weight: FontWeight(650.0),
            fallbacks: Some(FontFallbacks::from_fonts(vec![
                "Bahnschrift Condensed".into(),
            ])),
            ..Default::default()
        })?;
        // The Inter subset has no glyph for U+00E9.
        let font_ids = layout_font_ids(&text_system, "a\u{e9}", &[inter, inter]);
        let fallback = font_ids[1];
        assert_ne!(fallback, inter);
        assert_eq!(
            (
                axis_value(&text_system, fallback, DWRITE_FONT_AXIS_TAG_WIDTH)?,
                axis_value(&text_system, fallback, DWRITE_FONT_AXIS_TAG_WEIGHT)?,
            ),
            (75.0, 650.0)
        );
        assert_eq!(
            layout_font_ids(&text_system, "\u{e9}", &[fallback]),
            [fallback]
        );
        Ok(())
    }

    /// The font id of the fallback that draws the second character of the
    /// text, with the font of the family, the weight and the style.
    fn fallback_font_id(
        text_system: &DirectWriteTextSystem,
        text: &str,
        family: &str,
        weight: FontWeight,
        style: FontStyle,
        fallback: &str,
    ) -> Result<FontId> {
        let font_id = text_system.font_id(&Font {
            family: family.into(),
            weight,
            style,
            fallbacks: Some(FontFallbacks::from_fonts(vec![fallback.into()])),
            ..Default::default()
        })?;
        let font_ids = layout_font_ids(text_system, text, &[font_id, font_id]);
        assert_eq!(font_ids.len(), 2, "{family}, {fallback}");
        assert_ne!(font_ids[1], font_id, "{family}, {fallback}");
        Ok(font_ids[1])
    }

    // DirectWrite simulates a fallback face from the axis values of the run.
    // Inter goes up to the weight 900 and Consolas up to 700, and Segoe UI
    // has the weight 700 and MV Boli only 400. MV Boli has a slant of its
    // own. The Inter subset has no glyph for U+00E9, and Segoe UI and Arial
    // have none for U+0780.
    #[test]
    fn fallback_face_takes_the_simulations_of_its_match() -> Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        text_system.add_fonts(vec![Cow::Borrowed(INTER_VARIABLE)])?;
        for (text, family, weight, style, fallback, expected) in [
            (
                "a\u{e9}",
                "Inter Variable",
                FontWeight::BLACK,
                FontStyle::Normal,
                "Consolas",
                DWRITE_FONT_SIMULATIONS_NONE,
            ),
            (
                "a\u{780}",
                "Segoe UI",
                FontWeight::BOLD,
                FontStyle::Normal,
                "MV Boli",
                DWRITE_FONT_SIMULATIONS_BOLD,
            ),
            (
                "a\u{780}",
                "Arial",
                FontWeight::NORMAL,
                FontStyle::Italic,
                "MV Boli",
                DWRITE_FONT_SIMULATIONS_NONE,
            ),
        ] {
            let fallback = fallback_font_id(&text_system, text, family, weight, style, fallback)?;
            assert_eq!(
                simulations(&text_system, fallback),
                expected,
                "{family}, {fallback:?}"
            );
            let text = &text[1..];
            assert_eq!(layout_font_ids(&text_system, text, &[fallback]), [fallback]);
        }
        Ok(())
    }

    // A layout draws a missing character of each run with the fallbacks of
    // the font of that run, as a layout of that font alone does. The Inter
    // subset has no glyph for U+00E9, and Consolas and IBM Plex Sans have
    // none for U+2318.
    #[test]
    fn each_run_takes_the_fallbacks_of_its_font() -> Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        text_system.add_fonts(vec![
            Cow::Borrowed(INTER_VARIABLE),
            Cow::Borrowed(IBM_PLEX_SANS),
        ])?;
        let font = |family: &str, weight, style, fallbacks: &[&str]| {
            text_system.font_id(&Font {
                family: family.into(),
                weight,
                style,
                fallbacks: (!fallbacks.is_empty()).then(|| {
                    FontFallbacks::from_fonts(fallbacks.iter().map(|&name| name.into()).collect())
                }),
                ..Default::default()
            })
        };
        let (normal, medium, bold) = (FontWeight::NORMAL, FontWeight::MEDIUM, FontWeight::BOLD);
        let (upright, italic) = (FontStyle::Normal, FontStyle::Italic);
        let inter = |weight, style| font("Inter Variable", weight, style, &["Consolas"]);
        let consolas = |weight| font("Consolas", weight, upright, &["Inter Variable"]);
        for (character, first, second) in [
            ('\u{e9}', inter(normal, upright)?, inter(bold, upright)?),
            ('\u{e9}', inter(bold, upright)?, inter(normal, upright)?),
            ('\u{e9}', inter(normal, italic)?, inter(normal, upright)?),
            ('\u{e9}', inter(normal, upright)?, inter(normal, italic)?),
            ('\u{2318}', consolas(normal)?, consolas(bold)?),
            // Both fonts match Consolas Regular, and each matches its own
            // weight of Inter.
            ('\u{2318}', consolas(normal)?, consolas(medium)?),
            ('\u{2318}', consolas(medium)?, consolas(normal)?),
            (
                '\u{2318}',
                consolas(normal)?,
                font("Consolas", bold, upright, &[])?,
            ),
            (
                '\u{2318}',
                font("IBM Plex Sans", normal, upright, &["Inter Variable"])?,
                font("Consolas", normal, upright, &[])?,
            ),
        ] {
            assert_ne!(first, second);
            let text = format!("a{character}");
            let alone = |font_id| layout_font_ids(&text_system, &text, &[font_id, font_id]);
            assert_eq!(
                layout_font_ids(
                    &text_system,
                    &text.repeat(2),
                    &[first, first, second, second]
                ),
                [alone(first), alone(second)].concat(),
                "{first:?}, {second:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn fallback_matches_the_weight_of_the_font() -> Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        text_system.add_fonts(vec![Cow::Borrowed(INTER_VARIABLE)])?;
        // DirectWrite ranks Calibri Light made bold first for the weight 600.
        let inter = text_system.font_id(&Font {
            family: "Inter Variable".into(),
            weight: FontWeight::SEMIBOLD,
            fallbacks: Some(FontFallbacks::from_fonts(vec!["Calibri".into()])),
            ..Default::default()
        })?;
        // The Inter subset has no glyph for U+00E9.
        let calibri = layout_font_ids(&text_system, "a\u{e9}", &[inter, inter])[1];
        assert_eq!(full_name(&text_system, calibri)?, "Calibri Bold");
        assert_eq!(
            simulations(&text_system, calibri),
            DWRITE_FONT_SIMULATIONS_NONE
        );
        // Consolas has no face of the weight 500, and the font takes the
        // weight 400. The variable fallback takes 500.
        let consolas = text_system.font_id(&Font {
            family: "Consolas".into(),
            weight: FontWeight::MEDIUM,
            fallbacks: Some(FontFallbacks::from_fonts(vec!["Inter Variable".into()])),
            ..Default::default()
        })?;
        // Consolas has no glyph for U+2318.
        let inter = layout_font_ids(&text_system, "a\u{2318}", &[consolas, consolas])[1];
        assert_ne!(inter, consolas);
        assert_eq!(
            axis_value(&text_system, inter, DWRITE_FONT_AXIS_TAG_WEIGHT)?,
            500.0
        );
        for font_id in [calibri, inter] {
            assert_eq!(layout_font_ids(&text_system, "a", &[font_id]), [font_id]);
        }
        Ok(())
    }

    #[test]
    fn match_prefers_a_face_that_needs_no_simulation() -> Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        // DirectWrite ranks Calibri Light made bold first for the weight 600.
        let calibri = text_system.font_id(&Font {
            family: "Calibri".into(),
            weight: FontWeight::SEMIBOLD,
            ..Default::default()
        })?;
        assert_eq!(full_name(&text_system, calibri)?, "Calibri Bold");
        assert_eq!(
            simulations(&text_system, calibri),
            DWRITE_FONT_SIMULATIONS_NONE
        );
        // DirectWrite ranks Consolas Bold made bolder first for the weight 900.
        let consolas = text_system.font_id(&Font {
            family: "Consolas".into(),
            weight: FontWeight::BLACK,
            ..Default::default()
        })?;
        assert_eq!(full_name(&text_system, consolas)?, "Consolas Bold");
        assert_eq!(
            simulations(&text_system, consolas),
            DWRITE_FONT_SIMULATIONS_NONE
        );
        let mut font_ids = vec![calibri, consolas];
        // DirectWrite ranks Franklin Gothic Medium made bold before Franklin
        // Gothic Demi and Heavy for the weight 700. Not every system has
        // these bold faces.
        let franklin_gothic = |weight| {
            text_system.font_id(&Font {
                family: "Franklin Gothic".into(),
                weight,
                ..Default::default()
            })
        };
        let demi = franklin_gothic(FontWeight::SEMIBOLD)?;
        if full_name(&text_system, demi)? == "Franklin Gothic Demi" {
            let bold = franklin_gothic(FontWeight::BOLD)?;
            assert!(axis_value(&text_system, bold, DWRITE_FONT_AXIS_TAG_WEIGHT)? >= 600.0);
            assert_eq!(
                simulations(&text_system, bold),
                DWRITE_FONT_SIMULATIONS_NONE
            );
            font_ids.push(bold);
        }
        for font_id in font_ids {
            assert_eq!(layout_font_ids(&text_system, "a", &[font_id]), [font_id]);
        }
        Ok(())
    }

    #[test]
    fn match_prefers_a_real_slanted_face() -> Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        let font_id = |family: &str, weight, style| {
            text_system.font_id(&Font {
                family: family.into(),
                weight,
                style,
                ..Default::default()
            })
        };
        // DirectWrite ranks Arial made oblique before Arial Italic.
        let oblique = font_id("Arial", FontWeight::NORMAL, FontStyle::Oblique)?;
        assert_eq!(full_name(&text_system, oblique)?, "Arial Italic");
        // Arial Black has no italic face.
        let black = font_id("Arial", FontWeight::BLACK, FontStyle::Italic)?;
        assert_eq!(full_name(&text_system, black)?, "Arial Bold Italic");
        // MV Boli has a slant of its own.
        let mv_boli = font_id("MV Boli", FontWeight::NORMAL, FontStyle::Italic)?;
        for font_id in [oblique, black, mv_boli] {
            assert_eq!(
                simulations(&text_system, font_id),
                DWRITE_FONT_SIMULATIONS_NONE
            );
            assert_eq!(layout_font_ids(&text_system, "a", &[font_id]), [font_id]);
        }
        Ok(())
    }

    // Segoe UI has the weights 300, 350, 400, 600, 700 and 900.
    #[test]
    fn match_takes_the_weight_in_the_order_of_css() -> Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        for (weight, name) in [
            (100.0, "Segoe UI Light"),
            (350.0, "Segoe UI Semilight"),
            (380.0, "Segoe UI Semilight"),
            (450.0, "Segoe UI"),
            (500.0, "Segoe UI"),
            (550.0, "Segoe UI Semibold"),
            (800.0, "Segoe UI Black"),
        ] {
            let font_id = text_system.font_id(&Font {
                family: "Segoe UI".into(),
                weight: FontWeight(weight),
                ..Default::default()
            })?;
            assert_eq!(full_name(&text_system, font_id)?, name, "{weight}");
        }
        Ok(())
    }

    #[test]
    fn full_name_finds_its_face() -> Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        let font_id = text_system.font_id(&Font {
            family: "Arial Bold".into(),
            ..Default::default()
        })?;
        assert_eq!(full_name(&text_system, font_id)?, "Arial Bold");
        assert_eq!(
            simulations(&text_system, font_id),
            DWRITE_FONT_SIMULATIONS_NONE
        );
        assert_eq!(layout_font_ids(&text_system, "a", &[font_id]), [font_id]);
        Ok(())
    }

    #[test]
    fn fonts_that_share_a_face_keep_their_font_ids() -> Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        text_system.add_fonts(vec![Cow::Borrowed(INTER_VARIABLE)])?;
        let inter = Font {
            family: "Inter Variable".into(),
            ..Default::default()
        };
        let plain = text_system.font_id(&inter)?;
        let no_contextual_alternates = text_system.font_id(&Font {
            features: FontFeatures(Arc::new(vec![("calt".into(), 0)])),
            ..inter
        })?;
        assert_ne!(plain, no_contextual_alternates);
        for font_ids in [
            [plain, no_contextual_alternates],
            [no_contextual_alternates, plain],
        ] {
            assert_eq!(layout_font_ids(&text_system, "ab", &font_ids), font_ids);
        }
        // Fonts that match one face with the same features share a font id.
        // Consolas has the weights 400 and 700, and MV Boli only 400, which
        // gets a bold simulation for each weight from 600.
        let font_id = |family: &str, weight| {
            text_system.font_id(&Font {
                family: family.into(),
                weight: FontWeight(weight),
                ..Default::default()
            })
        };
        assert_eq!(font_id("Consolas", 600.0)?, font_id("Consolas", 700.0)?);
        assert_eq!(font_id("MV Boli", 700.0)?, font_id("MV Boli", 900.0)?);
        Ok(())
    }

    #[test]
    fn name_falls_back_to_the_first_one() -> Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        let components = &text_system.components;
        let family = HSTRING::from("Inter Test");
        let collection = unsafe {
            let file = components
                .in_memory_loader
                .CreateInMemoryFontFileReference(
                    &components.factory,
                    INTER_VARIABLE.as_ptr().cast(),
                    INTER_VARIABLE.len() as _,
                    None,
                )?;
            let builder = components.factory.CreateFontSetBuilder()?;
            builder.AddFont(
                &file,
                0,
                DWRITE_FONT_SIMULATIONS_NONE,
                &[],
                &[],
                &[DWRITE_FONT_PROPERTY {
                    propertyId: DWRITE_FONT_PROPERTY_ID_TYPOGRAPHIC_FAMILY_NAME,
                    propertyValue: PCWSTR(family.as_ptr()),
                    localeName: w!("ja-jp"),
                }],
            )?;
            font_collection(&components.factory, &builder.CreateFontSet()?)?
        };
        // The names can read the memory of the collection. Keep the
        // collection alive while the test reads them.
        let names = unsafe { collection.GetFontFamily(0)?.GetFamilyNames()? };
        assert_eq!(get_name(names, &HSTRING::from("en-us"))?, "Inter Test");
        Ok(())
    }

    #[test]
    fn face_in_no_collection_gets_a_font_id() -> Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        let components = &text_system.components;
        let face: IDWriteFontFace3 = unsafe {
            let file = components
                .in_memory_loader
                .CreateInMemoryFontFileReference(
                    &components.factory,
                    INTER_VARIABLE.as_ptr().cast(),
                    INTER_VARIABLE.len() as _,
                    None,
                )?;
            components
                .factory
                .CreateFontFace(
                    DWRITE_FONT_FACE_TYPE_TRUETYPE,
                    &[Some(file)],
                    0,
                    DWRITE_FONT_SIMULATIONS_NONE,
                )?
                .cast()?
        };
        let font_id = text_system
            .state
            .write()
            .drawn_font_id(components, None, &face, || Ok(Default::default()))
            .context("The face got no font id")?;
        assert!(*text_system.state.read().fonts[font_id.0].face() == face);
        assert_eq!(layout_font_ids(&text_system, "a", &[font_id]), [font_id]);
        Ok(())
    }

    #[test]
    fn added_font_becomes_fallback_of_resolved_font() -> Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        text_system.add_fonts(vec![Cow::Borrowed(IBM_PLEX_SANS)])?;
        let plex_font = Font {
            family: "IBM Plex Sans".into(),
            fallbacks: Some(FontFallbacks::from_fonts(vec!["Inter Variable".into()])),
            ..Default::default()
        };
        text_system.font_id(&plex_font)?;
        text_system.add_fonts(vec![Cow::Borrowed(INTER_VARIABLE)])?;
        let plex = text_system.font_id(&plex_font)?;
        let inter = text_system.font_id(&Font {
            family: "Inter Variable".into(),
            ..Default::default()
        })?;
        // IBM Plex Sans has no glyph for U+2318.
        assert_eq!(
            layout_font_ids(&text_system, "a\u{2318}", &[plex, plex]),
            [plex, inter]
        );
        Ok(())
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn a_variable_face_draws_each_requested_weight() -> Result<()> {
        gpui::variable_font_conformance::draws_each_requested_weight(&DirectWriteTextSystem::new()?)
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn a_variable_face_draws_the_nearest_weight_of_its_axis() -> Result<()> {
        gpui::variable_font_conformance::draws_the_nearest_weight_of_the_axis(
            &DirectWriteTextSystem::new()?,
        )
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn a_face_draws_the_nearest_whole_weight() -> Result<()> {
        gpui::variable_font_conformance::draws_the_nearest_whole_weight(Arc::new(
            DirectWriteTextSystem::new()?,
        ))
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn a_fallback_face_draws_at_the_weight_of_its_run() -> Result<()> {
        gpui::variable_font_conformance::fallback_draws_at_the_weight_of_its_run(
            &DirectWriteTextSystem::new()?,
        )
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn a_variable_face_draws_the_default_optical_size_at_each_size() -> Result<()> {
        gpui::variable_font_conformance::draws_the_default_optical_size(
            &DirectWriteTextSystem::new()?,
        )
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn many_weights_in_many_sizes_load_one_font_per_drawn_weight() -> Result<()> {
        gpui::variable_font_conformance::loads_one_font_per_drawn_weight(
            &DirectWriteTextSystem::new()?,
        )
    }

    #[test]
    fn adding_fonts_keeps_the_ids_of_unchanged_fonts() -> Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        let font = Font {
            family: "Segoe UI".into(),
            ..Default::default()
        };
        let font_id = text_system.font_id(&font)?;
        text_system.add_fonts(vec![Cow::Borrowed(INTER_VARIABLE)])?;
        assert_eq!(text_system.font_id(&font)?, font_id);
        assert_eq!(text_system.state.read().fonts.len(), 1);
        Ok(())
    }
}
