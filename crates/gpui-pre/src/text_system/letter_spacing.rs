//! Letter spacing of a shaped line.
//!
//! The model follows CSS Text Module Level 3, section 7.2 "Tracking: the
//! `letter-spacing` property" (W3C, editor's draft):
//! <https://drafts.csswg.org/css-text-3/#letter-spacing-property>
//!
//! - The unit of spacing is a grapheme cluster.
//! - A line has no gap before its first unit or after its last unit.
//! - A unit with no advance, such as a lone combining mark, gets no gap.
//!
//! Chromium skips zero-advance glyphs in the same way, in
//! `ShapeResult::ApplySpacingImpl`:
//! <https://chromium.googlesource.com/chromium/src/+/refs/heads/main/third_party/blink/renderer/platform/fonts/shaping/shape_result.cc>

use super::LineLayout;
use crate::{Em, Pixels, px};
use unicode_segmentation::UnicodeSegmentation as _;

/// Moves the glyphs of `layout` apart by `letter_spacing` at the font size of the layout,
/// and returns where the gaps went.
///
/// `text` is the text of the layout. The glyphs keep their visual order.
pub(crate) fn apply_letter_spacing(
    layout: &mut LineLayout,
    text: &str,
    letter_spacing: Em,
) -> LetterGaps {
    let gaps = LetterGaps::new(layout, text, letter_spacing);
    gaps.apply(layout);
    gaps
}

/// The gaps that letter spacing puts between the glyphs of a line.
#[derive(Clone, Debug, Default)]
pub(crate) struct LetterGaps {
    gap: Pixels,
    /// For each glyph of the line in visual order, whether a gap comes before it.
    /// Empty when the line has no spacing.
    before: Vec<bool>,
}

impl LetterGaps {
    /// The gaps of a line with no spacing.
    pub(crate) const NONE: Self = Self {
        gap: Pixels::ZERO,
        before: Vec::new(),
    };

    /// Finds the gaps of `letter_spacing` in `layout`, an unspaced layout of `text`.
    fn new(layout: &LineLayout, text: &str, letter_spacing: Em) -> Self {
        let gap = letter_spacing.to_pixels(layout.font_size);
        if gap == px(0.) {
            return Self::NONE;
        }
        let grapheme_starts = text
            .grapheme_indices(true)
            .map(|(start, _)| start)
            .collect::<Vec<_>>();
        let grapheme_start = |index: usize| {
            let ix = grapheme_starts.partition_point(|start| *start <= index);
            grapheme_starts[ix.saturating_sub(1)]
        };

        // A unit is a run of consecutive glyphs with the same grapheme start.
        let glyphs = layout
            .runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .collect::<Vec<_>>();
        let mut before = vec![false; glyphs.len()];
        let mut has_advanced = false;
        let mut start = 0;
        for unit in glyphs.chunk_by(|a, b| grapheme_start(a.index) == grapheme_start(b.index)) {
            let next = start + unit.len();
            let end = glyphs
                .get(next)
                .map_or(layout.width, |glyph| glyph.position.x);
            if end > unit[0].position.x {
                before[start] = has_advanced;
                has_advanced = true;
            }
            start = next;
        }
        Self { gap, before }
    }

    fn apply(&self, layout: &mut LineLayout) {
        let mut offset = px(0.);
        let glyphs = layout.runs.iter_mut().flat_map(|run| &mut run.glyphs);
        for (glyph, before) in glyphs.zip(&self.before) {
            if *before {
                offset += self.gap;
            }
            glyph.position.x += offset;
        }
        layout.width += offset;
    }

    /// The x where the content before a glyph ends. The glyph is at `x` and has the
    /// index `ix` among the glyphs of the line in visual order.
    pub(crate) fn content_end_before(&self, ix: usize, x: Pixels) -> Pixels {
        match self.before.get(ix) {
            Some(true) => x - self.gap,
            _ => x,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontId, GlyphId, ShapedGlyph, ShapedRun, em, point};

    fn glyph(x: f32, index: usize) -> ShapedGlyph {
        ShapedGlyph {
            id: GlyphId(0),
            position: point(px(x), px(0.)),
            index,
            is_emoji: false,
        }
    }

    fn layout(runs: Vec<Vec<ShapedGlyph>>, width: f32, len: usize) -> LineLayout {
        LineLayout {
            font_size: px(10.),
            width: px(width),
            ascent: px(8.),
            descent: px(2.),
            runs: runs
                .into_iter()
                .map(|glyphs| ShapedRun {
                    font_id: FontId(0),
                    glyphs,
                })
                .collect(),
            len,
        }
    }

    /// Lays out each char of `text` as one glyph of 6 px.
    fn monospace(text: &str) -> LineLayout {
        let glyphs = text
            .char_indices()
            .enumerate()
            .map(|(ix, (index, _))| glyph(ix as f32 * 6., index))
            .collect();
        layout(vec![glyphs], text.chars().count() as f32 * 6., text.len())
    }

    /// Spaces a layout of 10 px, so 0.1 em is 1 px.
    fn spaced(mut layout: LineLayout, text: &str, letter_spacing: Em) -> LineLayout {
        apply_letter_spacing(&mut layout, text, letter_spacing);
        layout
    }

    fn xs(layout: &LineLayout) -> Vec<f32> {
        layout
            .runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .map(|glyph| f32::from(glyph.position.x))
            .collect()
    }

    #[test]
    fn spacing_goes_between_units_and_not_after_the_last() {
        let text = "abc";
        let layout = spaced(monospace(text), text, em(0.2));
        assert_eq!(xs(&layout), [0., 8., 16.]);
        assert_eq!(layout.width, px(22.));
    }

    #[test]
    fn negative_spacing_pulls_units_together() {
        let text = "abc";
        let layout = spaced(monospace(text), text, em(-0.1));
        assert_eq!(xs(&layout), [0., 5., 10.]);
        assert_eq!(layout.width, px(16.));
    }

    #[test]
    fn no_spacing_keeps_the_layout() {
        let text = "abc";
        let layout = spaced(monospace(text), text, Em::ZERO);
        assert_eq!(xs(&layout), [0., 6., 12.]);
        assert_eq!(layout.width, px(18.));
    }

    #[test]
    fn a_combining_mark_stays_with_its_base() {
        // `e` and U+0301 form one grapheme. The mark has its own glyph and index.
        let text = "e\u{301}x";
        let glyphs = vec![glyph(0., 0), glyph(1., 1), glyph(6., 3)];
        let layout = spaced(layout(vec![glyphs], 12., text.len()), text, em(0.2));
        assert_eq!(xs(&layout), [0., 1., 8.]);
        assert_eq!(layout.width, px(14.));
    }

    #[test]
    fn a_ligature_over_two_graphemes_is_one_unit() {
        // `f` and `i` form one glyph with the index of `f`.
        let text = "fix";
        let glyphs = vec![glyph(0., 0), glyph(10., 2)];
        let layout = spaced(layout(vec![glyphs], 16., text.len()), text, em(0.2));
        assert_eq!(xs(&layout), [0., 12.]);
        assert_eq!(layout.width, px(18.));
    }

    #[test]
    fn a_unit_with_no_advance_gets_no_gap() {
        // Each zero-width space forms a grapheme with no advance. Both move with `a`, and
        // the gap goes once, between `a` and `b`.
        let text = "a\u{200b}\u{200b}b";
        let glyphs = vec![glyph(0., 0), glyph(6., 1), glyph(6., 4), glyph(6., 7)];
        let layout = spaced(layout(vec![glyphs], 12., text.len()), text, em(0.2));
        assert_eq!(xs(&layout), [0., 6., 6., 8.]);
        assert_eq!(layout.width, px(14.));
    }

    #[test]
    fn right_to_left_glyphs_keep_their_visual_order() {
        // The glyphs of a right-to-left run come in visual order, with falling indices.
        let text = "\u{5d0}\u{5d1}\u{5d2}";
        let glyphs = vec![glyph(0., 4), glyph(6., 2), glyph(12., 0)];
        let layout = spaced(layout(vec![glyphs], 18., text.len()), text, em(0.2));
        assert_eq!(xs(&layout), [0., 8., 16.]);
        let indices = layout.runs[0]
            .glyphs
            .iter()
            .map(|glyph| glyph.index)
            .collect::<Vec<_>>();
        assert_eq!(indices, [4, 2, 0]);
        assert_eq!(layout.width, px(22.));
    }

    #[test]
    fn units_span_shaped_runs() {
        let text = "abcd";
        let runs = vec![
            vec![glyph(0., 0), glyph(6., 1)],
            vec![glyph(12., 2), glyph(18., 3)],
        ];
        let layout = spaced(layout(runs, 24., text.len()), text, em(0.2));
        assert_eq!(xs(&layout), [0., 8., 16., 24.]);
        assert_eq!(layout.width, px(30.));
    }

    #[test]
    fn hit_tests_land_on_the_nearest_unit() {
        let text = "abc";
        let layout = spaced(monospace(text), text, em(0.4));
        // `b` spans 10..16. The gap after it spans 16..20, and `c` starts at 20.
        assert_eq!(layout.index_for_x(px(17.)), Some(1));
        assert_eq!(layout.index_for_x(px(21.)), Some(2));
        assert_eq!(layout.closest_index_for_x(px(11.)), 1);
        assert_eq!(layout.closest_index_for_x(px(19.)), 2);
        assert_eq!(layout.x_for_index(2), px(20.));
    }
}
