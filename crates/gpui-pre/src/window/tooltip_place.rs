//! The place of a tooltip that follows the pointer.

use crate::{Bounds, Decorations, Pixels, Point, Size, point, px, size};

/// The part of the surface that the user sees as the window. A window that
/// draws its own frame keeps a shadow of `client_inset` on each side that
/// is not tiled. The shadow is part of the surface, but not of the window.
pub(super) fn visible_area(
    surface: Size<Pixels>,
    decorations: Decorations,
    client_inset: Option<Pixels>,
) -> Bounds<Pixels> {
    let (Decorations::Client { tiling }, Some(inset)) = (decorations, client_inset) else {
        return Bounds::new(Point::default(), surface);
    };
    let shadow = |tiled: bool| if tiled { px(0.) } else { inset };
    let (left, top) = (shadow(tiling.left), shadow(tiling.top));
    let (right, bottom) = (shadow(tiling.right), shadow(tiling.bottom));
    Bounds::new(
        point(left, top),
        size(surface.width - left - right, surface.height - top - bottom),
    )
}

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
    use crate::Tiling;

    fn shadowed(tiling: Tiling) -> Bounds<Pixels> {
        let surface = size(px(1040.), px(840.));
        visible_area(surface, Decorations::Client { tiling }, Some(px(20.)))
    }

    #[test]
    fn the_visible_area_leaves_out_the_shadow_on_each_side_that_is_not_tiled() {
        let area = shadowed(Tiling::default());
        assert_eq!(
            area,
            Bounds::new(point(px(20.), px(20.)), size(px(1000.), px(800.)))
        );
        let left = Tiling {
            left: true,
            top: true,
            bottom: true,
            right: false,
        };
        let area = shadowed(left);
        assert_eq!(
            area,
            Bounds::new(point(px(0.), px(0.)), size(px(1020.), px(840.)))
        );
        assert_eq!(shadowed(Tiling::tiled()).size, size(px(1040.), px(840.)));
        let surface = size(px(1040.), px(840.));
        let server = visible_area(surface, Decorations::Server, Some(px(20.)));
        assert_eq!(server, Bounds::new(Point::default(), surface));
        let unset = visible_area(
            surface,
            Decorations::Client {
                tiling: Tiling::default(),
            },
            None,
        );
        assert_eq!(unset, Bounds::new(Point::default(), surface));
    }

    #[test]
    fn a_tooltip_stays_in_the_visible_area_of_a_window_with_a_shadow() {
        let area = shadowed(Tiling::default());
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
    fn a_tooltip_with_no_room_on_either_side_moves_into_the_visible_area() {
        let area = shadowed(Tiling::default());
        let wide = size(px(900.), px(30.));
        let placed = tooltip_bounds(point(px(500.), px(300.)), wide, area);
        assert_eq!(placed.origin.x, px(120.));
        let wider = size(px(1100.), px(30.));
        let placed = tooltip_bounds(point(px(500.), px(300.)), wider, area);
        assert_eq!(placed.origin.x, px(20.));
    }
}
