//! The edges of a shadow. A shadow anti-aliases the edge of its shape
//! where the edge lies inside a device pixel, also where the center of that
//! pixel lies outside the shape. A drop shadow paints only outside its
//! element.

use gpui::{
    Bounds, ContentMask, Corners, DevicePixels, PlatformHeadlessRenderer, Point, ScaledPixels,
    Scene, Shadow, Size, hsla,
};
use gpui_wgpu::WgpuHeadlessRenderer;
use image::RgbaImage;

fn device(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: Point {
            x: ScaledPixels(x),
            y: ScaledPixels(y),
        },
        size: Size {
            width: ScaledPixels(width),
            height: ScaledPixels(height),
        },
    }
}

/// A white drop shadow of `element`, with the shape `shape`, that paints
/// inside `mask`.
fn shadow(
    shape: Bounds<ScaledPixels>,
    blur: f32,
    element: Bounds<ScaledPixels>,
    mask: Bounds<ScaledPixels>,
) -> Shadow {
    Shadow {
        order: 0,
        blur_radius: ScaledPixels(blur),
        bounds: shape,
        corner_radii: Corners::default(),
        content_mask: ContentMask { bounds: mask },
        color: hsla(0., 0., 1., 1.),
        element_bounds: element,
        element_corner_radii: Corners::default(),
        inset: 0,
        pad: 0,
    }
}

/// The image of `shadow` on black, or none when the host has no headless
/// renderer.
fn render(shadow: Shadow, width: i32, height: i32) -> anyhow::Result<Option<RgbaImage>> {
    let mut renderer = match WgpuHeadlessRenderer::new() {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("The host has no headless renderer ({error}). The test checks nothing.");
            return Ok(None);
        }
    };
    let mut scene = Scene::default();
    scene.insert_primitive(shadow);
    scene.finish();
    let size = Size {
        width: DevicePixels(width),
        height: DevicePixels(height),
    };
    Ok(Some(renderer.render_scene_to_image(&scene, size)?))
}

#[test]
fn a_shadow_covers_part_of_the_pixels_at_its_fractional_edges() -> anyhow::Result<()> {
    let (width, height) = (16, 10);
    // The left edge covers a quarter of column 4, the right edge a quarter
    // of column 12.
    let shape = device(4.75, 2.75, 7.5, 4.5);
    // The element lies apart from the shape, so the whole shape shows.
    let element = device(0., 0., 0., 0.);
    let mask = device(0., 0., width as f32, height as f32);
    let Some(image) = render(shadow(shape, 0., element, mask), width, height)? else {
        return Ok(());
    };
    let red = |x: u32| image.get_pixel(x, 5).0[0];
    let (empty, full) = (red(0), red(8));
    assert!(
        empty < full,
        "row 5: {:?}",
        (0..16).map(red).collect::<Vec<_>>()
    );
    for x in [3, 13] {
        assert_eq!(red(x), empty, "column {x}");
    }
    for x in [4, 12] {
        assert!(
            empty < red(x) && red(x) < full,
            "column {x}: {} is not between {empty} and {full}",
            red(x)
        );
    }
    Ok(())
}

#[test]
fn a_drop_shadow_paints_only_outside_its_element() -> anyhow::Result<()> {
    let (width, height) = (40, 40);
    // The shadow falls 2 pixels below its element, with a blur of 3.
    let element = device(10., 10., 20., 20.);
    let shape = device(10., 12., 20., 20.);
    let whole = device(0., 0., width as f32, height as f32);
    let Some(image) = render(shadow(shape, 3., element, whole), width, height)? else {
        return Ok(());
    };
    let red = |x: u32, y: u32| image.get_pixel(x, y).0[0];
    for (x, y) in [(11, 11), (20, 20), (28, 28)] {
        assert_eq!(red(x, y), 0, "inside the element at {x}, {y}");
    }
    for (x, y) in [(20, 8), (8, 20), (31, 20), (20, 32)] {
        assert!(red(x, y) > 0, "outside the element at {x}, {y}");
    }

    // The shape lies below the band above the element, and its blur
    // reaches into the band. The shadow paints only in the band.
    let band = device(0., 0., width as f32, 10.);
    let Some(image) = render(shadow(shape, 3., element, band), width, height)? else {
        return Ok(());
    };
    let red = |x: u32, y: u32| image.get_pixel(x, y).0[0];
    assert!(red(20, 9) > 0, "in the band");
    for (x, y) in [(8, 20), (20, 32)] {
        assert_eq!(red(x, y), 0, "below the band at {x}, {y}");
    }

    // A ring around an element whose left edge halves column 10. The ring
    // keeps its full alpha under that column.
    let element = device(10.5, 10., 19.5, 20.);
    let ring = device(8., 8., 24., 24.);
    let Some(image) = render(shadow(ring, 0., element, whole), width, height)? else {
        return Ok(());
    };
    assert_eq!(image.get_pixel(10, 20).0[0], 255, "under the edge");
    assert_eq!(image.get_pixel(12, 20).0[0], 0, "inside the element");
    Ok(())
}
