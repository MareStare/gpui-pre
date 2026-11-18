//! The place of a tooltip next to the pointer or next to the element that shows it.

use crate::{Bounds, Pixels, Point, Size, point, px};

/// The bounds of a tooltip of `size` for the pointer at `pointer`. The
/// tooltip starts 1px right of and below the pointer. On an axis where it
/// runs past `area`, it goes to the other side of the pointer. Where
/// neither side has room, it moves into `area`.
pub(super) fn at_pointer(
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

/// The bounds of a tooltip of `size` for the element at `anchor`. The
/// tooltip is centered under the element. When it runs past the bottom of
/// `area`, it goes above the element. When neither side has room, it moves
/// into `area`. It moves sideways to stay inside `area`. The view of the
/// tooltip holds its gap to the element as a margin.
pub(super) fn under_element(
    anchor: Bounds<Pixels>,
    size: Size<Pixels>,
    area: Bounds<Pixels>,
) -> Bounds<Pixels> {
    let centered = anchor.center().x - size.width / 2.;
    let x = centered.min(area.right() - size.width).max(area.left());
    let y = if anchor.bottom() + size.height <= area.bottom() {
        anchor.bottom()
    } else if anchor.top() - size.height >= area.top() {
        anchor.top() - size.height
    } else {
        (area.bottom() - size.height).max(area.top())
    };
    Bounds::new(point(x, y), size)
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
    fn a_tooltip_at_the_pointer_stays_in_the_frame_of_a_window_with_a_shadow() {
        let area = shadowed();
        let tip = size(px(120.), px(30.));
        let free = at_pointer(point(px(400.), px(300.)), tip, area);
        assert_eq!(free.origin, point(px(401.), px(301.)));
        // 12px above the bottom of the frame, 8px left of its right side.
        let corner = point(area.right() - px(8.), area.bottom() - px(12.));
        let flipped = at_pointer(corner, tip, area);
        assert_eq!(flipped.origin, point(px(891.), px(777.)));
        assert!(flipped.bottom() <= area.bottom(), "{flipped:?}");
        assert!(flipped.right() <= area.right(), "{flipped:?}");
    }

    #[test]
    fn a_tooltip_at_the_pointer_with_no_room_on_either_side_moves_into_the_frame() {
        let area = shadowed();
        let wide = size(px(900.), px(30.));
        let placed = at_pointer(point(px(500.), px(300.)), wide, area);
        assert_eq!(placed.origin.x, px(120.));
        let wider = size(px(1100.), px(30.));
        let placed = at_pointer(point(px(500.), px(300.)), wider, area);
        assert_eq!(placed.origin.x, px(20.));
    }

    fn button(x: f32, y: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(30.), px(20.)))
    }

    #[test]
    fn a_tooltip_sits_centered_under_its_element() {
        let tip = size(px(120.), px(30.));
        let placed = under_element(button(400., 300.), tip, shadowed());
        assert_eq!(placed.origin, point(px(355.), px(320.)));
    }

    #[test]
    fn a_tooltip_with_no_room_below_goes_above_its_element() {
        let area = shadowed();
        let tip = size(px(120.), px(30.));
        let placed = under_element(button(400., area.bottom().0 - 30.), tip, area);
        assert_eq!(placed.origin, point(px(355.), area.bottom() - px(60.)));
    }

    #[test]
    fn a_tooltip_stays_in_the_frame_of_a_window_with_a_shadow() {
        let area = shadowed();
        let tip = size(px(120.), px(30.));
        let right = under_element(button(area.right().0 - 30., 300.), tip, area);
        assert_eq!(right.right(), area.right());
        let left = under_element(button(area.left().0, 300.), tip, area);
        assert_eq!(left.left(), area.left());
    }

    #[test]
    fn a_tooltip_with_no_room_on_either_side_moves_into_the_frame() {
        let area = shadowed();
        let tall = size(px(120.), px(500.));
        let placed = under_element(button(400., 400.), tall, area);
        assert_eq!(placed.bottom(), area.bottom());
        let wider = size(px(1100.), px(30.));
        let placed = under_element(button(400., 300.), wider, area);
        assert_eq!(placed.origin.x, area.left());
    }
}
