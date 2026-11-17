//! Checks of the weights of a variable font that the text system of each
//! platform must pass. Each check takes a text system with no added fonts and
//! adds the test fonts to it.

use crate::{
    Font, FontFallbacks, FontId, FontRun, FontWeight, PlatformTextSystem, RenderGlyphParams,
    TextSystem, font, px,
};
use anyhow::{Context as _, Result};
use itertools::Itertools as _;
use std::{borrow::Cow, path::Path, sync::Arc};

/// The family of a variable test font with a weight axis from 100 to 900.
pub const VARIABLE_FAMILY: &str = "Inter Variable";

/// The family of two static test fonts, a regular one and a bold one, with no
/// U+2318. The variable test font has U+2318.
pub const STATIC_FAMILY: &str = "Lilex";

/// Adds the fonts of `VARIABLE_FAMILY` and `STATIC_FAMILY`. The files are not
/// part of the package of the crate, so the checks read them at run time.
pub fn add_fonts(text_system: &dyn PlatformTextSystem) -> Result<()> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts");
    let fonts = [
        "inter/InterVariable-subset.ttf",
        "lilex/Lilex-Regular.ttf",
        "lilex/Lilex-Bold.ttf",
    ]
    .into_iter()
    .map(|file| {
        let path = dir.join(file);
        std::fs::read(&path)
            .map(Cow::Owned)
            .with_context(|| format!("cannot read {}", path.display()))
    })
    .collect::<Result<_>>()?;
    text_system.add_fonts(fonts)
}

/// The font of `family` at `weight`.
pub fn weighted(family: &str, weight: f32) -> Font {
    Font {
        weight: FontWeight(weight),
        ..font(family.to_owned())
    }
}

/// The parameters of the raster of `ch` at `size` pixels.
pub fn glyph_params(
    text_system: &dyn PlatformTextSystem,
    font_id: FontId,
    ch: char,
    size: f32,
) -> Result<RenderGlyphParams> {
    Ok(RenderGlyphParams {
        font_id,
        glyph_id: text_system
            .glyph_for_char(font_id, ch)
            .with_context(|| format!("the font has no {ch:?}"))?,
        font_size: px(size),
        subpixel_variant: Default::default(),
        scale_factor: 1.0,
        is_emoji: false,
        subpixel_rendering: false,
        dilation: 0,
    })
}

/// The sum of the coverage of the raster of `ch` at 32 pixels. A heavier
/// weight puts more ink on the glyph.
pub fn ink(text_system: &dyn PlatformTextSystem, font_id: FontId, ch: char) -> Result<u64> {
    let params = glyph_params(text_system, font_id, ch, 32.0)?;
    let bounds = text_system.glyph_raster_bounds(&params)?;
    let (_, alpha) = text_system.rasterize_glyph(&params, bounds)?;
    Ok(alpha.iter().map(|&alpha| u64::from(alpha)).sum())
}

/// Each weight gets its own font. A heavier font has a wider advance, more
/// ink and a wider layout, and the layout keeps the requested font.
pub fn draws_each_requested_weight(text_system: &dyn PlatformTextSystem) -> Result<()> {
    add_fonts(text_system)?;
    let weights = [100.0, 400.0, 500.0, 550.0, 600.0, 700.0, 900.0];
    let font_ids = weights
        .iter()
        .map(|&weight| text_system.font_id(&weighted(VARIABLE_FAMILY, weight)))
        .collect::<Result<Vec<_>>>()?;
    assert!(font_ids.iter().all_unique(), "{font_ids:?}");

    let advances = font_ids
        .iter()
        .map(|&font_id| {
            let glyph = text_system
                .glyph_for_char(font_id, 'm')
                .context("the font has no 'm'")?;
            Ok(text_system.advance(font_id, glyph)?.width)
        })
        .collect::<Result<Vec<_>>>()?;
    assert!(advances.is_sorted_by(|a, b| a < b), "{advances:?}");

    let inks = font_ids
        .iter()
        .map(|&font_id| ink(text_system, font_id, 'm'))
        .collect::<Result<Vec<_>>>()?;
    assert!(inks.is_sorted_by(|a, b| a < b), "{inks:?}");

    let widths = font_ids
        .iter()
        .map(|&font_id| {
            let runs = [FontRun { len: 1, font_id }];
            let layout = text_system.layout_line("m", px(16.0), &runs);
            let run_font_ids = layout.runs.iter().map(|run| run.font_id).collect_vec();
            assert_eq!(run_font_ids, [font_id]);
            layout.width
        })
        .collect_vec();
    assert!(widths.is_sorted_by(|a, b| a < b), "{widths:?}");
    Ok(())
}

/// A weight outside the weight axis draws the nearest end of the axis.
pub fn draws_the_nearest_weight_of_the_axis(text_system: &dyn PlatformTextSystem) -> Result<()> {
    add_fonts(text_system)?;
    let font_id = |weight| text_system.font_id(&weighted(VARIABLE_FAMILY, weight));
    assert_eq!(font_id(50.0)?, font_id(100.0)?);
    assert_eq!(font_id(950.0)?, font_id(900.0)?);
    assert_eq!(font_id(-1.0)?, font_id(100.0)?);
    assert_eq!(font_id(1001.0)?, font_id(900.0)?);
    Ok(())
}

/// A weight between two integers draws the nearest integer, in a variable
/// family and in a static family. A weight halfway between rounds up.
pub fn draws_the_nearest_whole_weight(
    platform_text_system: Arc<dyn PlatformTextSystem>,
) -> Result<()> {
    add_fonts(platform_text_system.as_ref())?;
    let text_system = TextSystem::new(platform_text_system);
    for (family, weight) in [(VARIABLE_FAMILY, 400.0), (STATIC_FAMILY, 500.0)] {
        assert_eq!(
            text_system.resolve_font(&weighted(family, weight + 0.4)),
            text_system.resolve_font(&weighted(family, weight)),
            "{family}"
        );
    }
    assert_eq!(
        text_system.resolve_font(&weighted(STATIC_FAMILY, 500.5)),
        text_system.resolve_font(&weighted(STATIC_FAMILY, 501.0))
    );
    Ok(())
}

/// Requests of each integer weight from 0 to 1000 get one font for each
/// integer weight of the axis, and the same fonts on each pass. Rasters in
/// many sizes do not change the fonts.
pub fn loads_one_font_per_drawn_weight(text_system: &dyn PlatformTextSystem) -> Result<()> {
    add_fonts(text_system)?;
    let mut passes = Vec::new();
    for _ in 0..2 {
        let mut font_ids = Vec::new();
        for weight in 0..=1000 {
            let font_id = text_system.font_id(&weighted(VARIABLE_FAMILY, weight as f32))?;
            if weight % 50 == 0 {
                for size in (6..=96).step_by(6) {
                    let params = glyph_params(text_system, font_id, 'm', size as f32)?;
                    let bounds = text_system.glyph_raster_bounds(&params)?;
                    text_system.rasterize_glyph(&params, bounds)?;
                }
            }
            font_ids.push(font_id);
        }
        assert_eq!(font_ids.iter().unique().count(), 801);
        passes.push(font_ids);
    }
    assert_eq!(passes[0], passes[1]);
    Ok(())
}

/// A face of the fallbacks of a font draws at the weight of its run.
pub fn fallback_draws_at_the_weight_of_its_run(text_system: &dyn PlatformTextSystem) -> Result<()> {
    add_fonts(text_system)?;
    let variable_bold = text_system.font_id(&weighted(VARIABLE_FAMILY, 700.0))?;
    let static_bold = text_system.font_id(&Font {
        fallbacks: Some(FontFallbacks::from_fonts(vec![VARIABLE_FAMILY.to_owned()])),
        ..weighted(STATIC_FAMILY, 700.0)
    })?;
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
