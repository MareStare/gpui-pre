//! The place of a tooltip that follows the pointer.

use crate::{Bounds, Pixels, Point, Size, point, px};

/// The bounds of a tooltip of `size` for the pointer at `pointer`. The
/// tooltip starts 1px right of and below the pointer. On an axis where it
/// runs past `area`, it goes to the other side of the pointer. Where
/// neither side has room, it moves into `area`.
pub(super) fn tooltip_bounds(
    pointer: Point<Pixels>,
    size: Size<Pixels>,
    area: Bounds<Pixels>,
) -> Bounds<Pixels> {
    let x = place_on_axis(pointer.x, size.width, area.left(), area.right());
    let y = place_on_axis(pointer.y, size.height, area.top(), area.bottom());
    Bounds::new(point(x, y), size)
}

fn place_on_axis(pointer: Pixels, length: Pixels, start: Pixels, end: Pixels) -> Pixels {
    let after = pointer + px(1.);
    if after + length <= end {
        return after;
    }
    let before = pointer - length - px(1.);
    if before >= start {
        return before;
    }
    (end - length).max(start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tiling, size};

    /// The frame of a window of 1000 by 800 px with a shadow of 20 px.
    fn shadowed() -> Bounds<Pixels> {
        let surface = Bounds::new(Point::default(), size(px(1040.), px(840.)));
        Tiling::default().inset(surface, px(20.))
    }

    #[test]
    fn a_tooltip_stays_in_the_frame_of_a_window_with_a_shadow() {
        let area = shadowed();
        let tip = size(px(120.), px(30.));
        let free = tooltip_bounds(point(px(400.), px(300.)), tip, area);
        assert_eq!(free.origin, point(px(401.), px(301.)));
        // 12px above the bottom of the frame, 8px left of its right side.
        let corner = point(area.right() - px(8.), area.bottom() - px(12.));
        let flipped = tooltip_bounds(corner, tip, area);
        assert_eq!(flipped.origin, point(px(891.), px(777.)));
        assert!(flipped.bottom() <= area.bottom(), "{flipped:?}");
        assert!(flipped.right() <= area.right(), "{flipped:?}");
    }

    #[test]
    fn a_tooltip_with_no_room_on_either_side_moves_into_the_frame() {
        let area = shadowed();
        let wide = size(px(900.), px(30.));
        let placed = tooltip_bounds(point(px(500.), px(300.)), wide, area);
        assert_eq!(placed.origin.x, px(120.));
        let wider = size(px(1100.), px(30.));
        let placed = tooltip_bounds(point(px(500.), px(300.)), wider, area);
        assert_eq!(placed.origin.x, px(20.));
    }
}
