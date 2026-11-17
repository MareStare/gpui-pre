# gpui-pre with patches

The [gpui-pre](https://crates.io/crates/gpui-pre) crates are a snapshot of
the GPUI crates of [Zed](https://github.com/zed-industries/zed), published
on crates.io. This repo holds the crates of that snapshot that the patches
change, under `crates/`. Use them in place of the crates.io versions:

```toml
[patch.crates-io]
gpui-pre         = { git = "https://github.com/MareStare/gpui-pre", rev = "<commit>" }
gpui-pre-apple   = { git = "https://github.com/MareStare/gpui-pre", rev = "<commit>" }
gpui-pre-linux   = { git = "https://github.com/MareStare/gpui-pre", rev = "<commit>" }
gpui-pre-macos   = { git = "https://github.com/MareStare/gpui-pre", rev = "<commit>" }
gpui-pre-wgpu    = { git = "https://github.com/MareStare/gpui-pre", rev = "<commit>" }
gpui-pre-windows = { git = "https://github.com/MareStare/gpui-pre", rev = "<commit>" }
```

## Build

The root workspace builds the crates together, with each one patched to
its copy here: `cargo build -p gpui-pre-wgpu`.

The unit tests of a crate need its `test-support` feature:
`cargo test -p gpui-pre-wgpu --features test-support`. The tests of
`gpui-pre-wgpu` read fonts of Zed from `assets/fonts`.

## Layout of the history

1. One commit adds the published sources of one gpui-pre version, with no
   changes.
2. Each patch is one commit on top of it, so each one rebases on its own
   onto a later gpui-pre version.
3. A commit with a `(cherry picked from commit <sha>)` line takes the
   change of that Zed commit, with the paths of the crates here. Drop it
   in a rebase onto a gpui-pre version that holds that Zed commit.

To move to a later version, add its published sources in a new base
commit, then rebase the patch commits onto it.

## Patches

- No examples: `gpui-pre` has no examples. It keeps the image of the
  examples that a unit test reads. A rebase deletes the examples that a
  later version adds. A dev-dependency on `gpui-pre-platform` from
  crates.io, with the platform crates of Linux, macOS and Windows patched
  to the copies here, builds the doc tests. It takes the runtime shaders,
  so the doc tests build on macOS with no Metal compiler. Its version pin
  equals the gpui-pre version: a rebase bumps it.
- Image sprite filter: an image sprite samples its texture with a filter
  that keeps thin lines when it is drawn smaller. The filter reads no
  texels of the images next to the sprite in the atlas.
- Custom surfaces: `Window::paint_custom_surface` paints a surface that
  code outside the renderer draws with wgpu. `gpui-pre-wgpu` exports
  `WgpuSurface`, `WgpuSurfaceFrame` and `wgpu_custom_surface`.
- wgpu by default on Windows and macOS: `GPUI_RENDERER=dx11` or
  `GPUI_RENDERER=metal` picks the native renderer. A window whose wgpu
  renderer fails to start uses the native renderer. On macOS the wgpu
  renderer draws to the `CAMetalLayer` of the view, in Display P3 when the
  window asks for it, and transparent windows show what is behind them.
  An unknown `GPUI_RENDERER` logs a warning and picks wgpu. On Windows the
  wgpu renderer takes the DirectWrite text contrast and ClearType glyphs
  as BGRA. On macOS the wgpu renderer tries the integrated GPU first.
- Startup log: `GPUI_STARTUP_LOG` logs the time of the startup stages of
  the renderers.
- Startup speed: the render pipelines build in parallel, wgpu does not
  validate indirect calls, DX12 starts only when no Vulkan adapter can
  draw to the window, and a resize does not wait for the GPU.
- Backends from the environment: `WGPU_BACKEND` and the wgpu backend
  options override the default, Vulkan and then DX12 on Windows.
- Device loss recovery: the wgpu window renderer of macOS and Windows makes
  a new device after a lost one, with a new identity, and asks the next
  frame to build the scene again. `Window::gpu_failure` gives the GPU
  error of a wgpu window that stays after the renderer dropped its GPU
  caches to recover. The renderer keeps trying to recover and does not
  panic.
- Adapter by name: the wgpu renderer tries first the adapter whose name
  contains `WGPU_ADAPTER_NAME`, case-insensitive. It ranks above the
  adapter of `ZED_DEVICE_ID`. A window panics when no adapter has the name.
- Cached views: a cached view keeps its window control areas, such as
  the drag area of a title bar, in the frames that reuse its paint. Tests
  run the hit test of the window control areas with
  `VisualTestContext::hit_test_window_control_area`.
- Headless apps: the `headless` feature runs an app on the test platform
  in real time and draws each frame with the wgpu headless renderer.
  `Window::named_element_bounds` gives the visible bounds of each named
  element. `Window::render_to_image` works in every build: the wgpu
  renderer draws the scene to an offscreen texture and reads it back, also
  for X11 and Wayland windows.
- Screenshots off the UI thread: `Window::render_to_pending_image` submits
  the render of the frame and returns a `PendingImage`. Its `wait` waits
  for the GPU and reads the pixels on any thread. The wgpu and Metal
  renderers implement it, and the headless renderers through
  `PlatformHeadlessRenderer::render_scene_to_pending_image`. wgpu puts
  the render and the copy to the CPU in one submission: on some drivers
  a second submission waits for the GPU to finish the first. It keeps
  the target and the readback buffer while the size stays the same: on
  some drivers the first copy to a new buffer costs milliseconds. It
  polls the device without a wait: a poll that waits holds a lock that
  the present of a frame on the UI thread needs. DirectX reads the image
  inside the call.
- Pen input on macOS and Linux: `Window::on_pen` gets each pen sample
  with its pressure and the proximity changes of the pen, before the
  mouse event of the same sample. macOS reads the tablet events of
  AppKit, and `on_pen` turns off mouse coalescing for the whole app.
  Wayland reads the tablet protocol and makes the mouse events of each
  tool. X11 reads the pressure valuator of XInput 2 and converts event
  positions from 16.16 fixed point by 65536.
- Image cursors: `Window::set_cursor_image` shows an RGBA image as the
  cursor of a hitbox, with a hotspot and a scale, on Windows, macOS, X11
  and Wayland. On Wayland the tablet tools show it too. A platform that
  cannot show the image shows the fallback style. The native cursor of an
  image lives as long as the image.
- Cursor warp: `Window::set_mouse_position` moves the mouse cursor of the
  system to a point of the window. Wayland uses `wp_pointer_warp_v1`
  while the pointer is over the window, and the compositor may refuse it.
- Pen input on Windows: `Window::on_pen` reads the pointer messages of
  Windows Ink, with the whole history of each message, and the packets
  of a WinTab context. `App::set_pen_api` picks Windows Ink, WinTab, or
  WinTab until Windows Ink gives the first touch of a pen. A thread
  other than the UI thread opens each WinTab context, so a driver that
  does not answer blocks no window.
- Quit on macOS: `Platform::quit` stops the AppKit run loop, and `run`
  calls the quit callback after it and returns, as on Linux and Windows.
  A quit that the system asks for, such as at logout, still ends the
  process with `exit` in `terminate:`.
- Wrapped rows fit: a row that is still too wide after a break at a
  break opportunity breaks again, so no row is wider than the wrap width
  when each glyph fits. The spaces at the end of a row hang past the edge,
  as in CSS. They get no row of their own and take no part in the alignment
  of the row.
- Typographic font families on Windows: DirectWrite groups the added fonts
  and the system fonts into typographic families. A lookup finds a family
  by its typographic, weight-stretch-style or GDI name, or a face by its
  full name, in the added fonts first. A name finds only the faces of its
  own family. It matches the weight and the style through the font axes,
  so a variable font takes any weight of its range. A variable font with
  an optical size axis, such as Inter 4, has a weight-stretch-style family
  name for each optical size, and its typographic name finds it. Of the
  matching fonts, the width nearest to the first font goes first. Then the
  style of the face, in the order of CSS, and a simulated oblique comes
  after each real slanted face. Then a bold request takes a bold face or a
  bold simulation, then the fewest simulations win, then the weight in the
  order of CSS. Each font id has a font collection with only its face,
  under a family name of its own, so a layout draws the face that the
  lookup picked, and each run gives back its own font id. Fonts with the
  same features and the same faces share a font id, also when they ask
  for other weights of one simulated face. Fallback fonts use the same
  lookup, with the weight and the style of the font. Each run of a layout
  draws the fallback faces that its own font matched, in the order of its
  fallbacks, with the simulations of each match. A variable fallback face
  can get a simulation when the run asks for a weight or a slant outside
  the range of its axes. A face in no collection gets a font id from the
  face itself.
  Adding fonts keeps the font ids of the fonts that resolve to the same
  faces. `all_font_names` lists the typographic names. The text system
  needs Windows 10 version 1709 or later.

## License

Apache-2.0, as the crates it holds. See `LICENSE-APACHE`.
