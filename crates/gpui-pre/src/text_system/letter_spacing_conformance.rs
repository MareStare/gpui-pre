//! Checks of letter spacing that the text system of each platform must pass.
//! Each check takes a text system with no added fonts and adds the test fonts
//! to it.

use super::variable_font_conformance::{STATIC_FAMILY, add_fonts};
use crate::{
    Em, Font, FontFeatures, GlyphId, Pixels, PlatformTextSystem, SharedString, TextRun, TextSystem,
    WindowTextSystem, em, font, px,
};
use anyhow::{Context as _, Result, ensure};
use std::sync::Arc;

/// Checks that a decomposed `é` keeps its mark with its base under letter
/// spacing, and that the gap goes after the mark.
pub fn combining_mark_keeps_its_base(text_system: Arc<dyn PlatformTextSystem>) -> Result<()> {
    add_fonts(text_system.as_ref())?;
    let text_system = WindowTextSystem::new(Arc::new(TextSystem::new(text_system)));
    let text = SharedString::from("e\u{301}x");
    let font_size = px(16.);
    let letter_spacing = em(0.25);
    let gap = letter_spacing.to_pixels(font_size);
    let shape = |letter_spacing| {
        let run = TextRun {
            len: text.len(),
            font: font(STATIC_FAMILY),
            ..Default::default()
        };
        text_system.shape_spaced_line(text.clone(), font_size, &[run], letter_spacing, None)
    };
    let plain = shape(Em::ZERO);
    let spaced = shape(letter_spacing);

    let glyphs = |line: &crate::ShapedLine| -> Vec<(usize, Pixels)> {
        line.layout
            .runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .map(|glyph| (glyph.index, glyph.position.x))
            .collect()
    };
    let plain_glyphs = glyphs(&plain);
    let spaced_glyphs = glyphs(&spaced);
    ensure!(
        plain_glyphs.len() == spaced_glyphs.len(),
        "glyphs differ: {plain_glyphs:?} and {spaced_glyphs:?}"
    );
    for ((index, plain_x), (_, spaced_x)) in plain_glyphs.iter().zip(&spaced_glyphs) {
        let expected = if *index < 3 { *plain_x } else { *plain_x + gap };
        ensure!(
            (*spaced_x - expected).abs() < px(0.01),
            "glyph of byte {index} at {spaced_x:?}, expected {expected:?}: \
             {plain_glyphs:?} and {spaced_glyphs:?}"
        );
    }
    ensure!(
        (spaced.width - plain.width - gap).abs() < px(0.01),
        "width {:?}, expected {:?}",
        spaced.width,
        plain.width + gap
    );
    Ok(())
}

/// Checks that spaced text takes no ligatures, unless its font features turn
/// them on.
pub fn spacing_turns_off_ligatures(text_system: Arc<dyn PlatformTextSystem>) -> Result<()> {
    add_fonts(text_system.as_ref())?;
    let platform_text_system = text_system.clone();
    let text_system = WindowTextSystem::new(Arc::new(TextSystem::new(text_system)));
    let text = SharedString::from("->");
    let glyph_ids = |font: &Font, letter_spacing: Em| -> Vec<GlyphId> {
        let run = TextRun {
            len: text.len(),
            font: font.clone(),
            ..Default::default()
        };
        text_system
            .shape_spaced_line(text.clone(), px(16.), &[run], letter_spacing, None)
            .layout
            .runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .map(|glyph| glyph.id)
            .collect()
    };
    let plain_font = font(STATIC_FAMILY);
    let font_id = platform_text_system.font_id(&plain_font)?;
    let separate = text
        .chars()
        .map(|character| platform_text_system.glyph_for_char(font_id, character))
        .collect::<Option<Vec<_>>>()
        .context("the test font has no glyph of the text")?;

    let ligature = glyph_ids(&plain_font, Em::ZERO);
    ensure!(
        ligature != separate,
        "the test font forms no ligature of {text:?}: {ligature:?}"
    );
    let spaced = glyph_ids(&plain_font, em(0.1));
    ensure!(
        spaced == separate,
        "spaced glyphs {spaced:?}, expected {separate:?}"
    );

    let ligatures_on = Font {
        features: FontFeatures(Arc::new(vec![("liga".into(), 1), ("calt".into(), 1)])),
        ..plain_font
    };
    let spaced = glyph_ids(&ligatures_on, em(0.1));
    ensure!(
        spaced == ligature,
        "spaced glyphs with ligatures on {spaced:?}, expected {ligature:?}"
    );
    Ok(())
}
