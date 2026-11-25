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

The unit tests of a crate need its `test-support` feature. The tests of
`gpui-pre-wgpu` also need `font-kit`, which `gpui-pre-linux` turns on:
`cargo test -p gpui-pre-wgpu --features test-support,font-kit`.
The tests of `gpui-pre-wgpu`, `gpui-pre-apple` and `gpui-pre-windows`
read fonts of Zed from `assets/fonts`.

## Layout of the history

`AGENTS.md` describes the branches, the tags and how an upgrade to a later
gpui-pre version goes.

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
  The renderer draws a surface on the UI thread, so its content needs no
  `Send` or `Sync`.
- wgpu by default on Windows and macOS: `GPUI_RENDERER=dx11` or
  `GPUI_RENDERER=metal` picks the native renderer. A window whose wgpu
  renderer fails to start uses the native renderer. On macOS the wgpu
  renderer draws to the `CAMetalLayer` of the view, in Display P3 when the
  window asks for it, and transparent windows show what is behind them.
  An unknown `GPUI_RENDERER` logs a warning and picks wgpu. On Windows the
  wgpu renderer takes the DirectWrite text contrast and ClearType glyphs
  as BGRA, and a change of the window background sets the alpha mode of the
  surface. The HWND swapchain of DX12 offers only an opaque alpha mode, and
  so do common Vulkan drivers, so a transparent, blurred or Mica window of
  the wgpu renderer draws opaque. `GPUI_RENDERER=dx11` draws these
  backgrounds. DX12 with `WGPU_DX12_PRESENTATION_SYSTEM=visual` offers a
  premultiplied alpha mode. A window of the wgpu renderer keeps its
  redirection surface, since some drivers present through it, so a
  Direct3D 11 fallback draws with no DirectComposition. On macOS the wgpu
  renderer tries the integrated GPU first.
- Present with no wait for a vertical blank: the wgpu window renderer of
  Windows and macOS prefers Mailbox, since the platform already paces the
  frames at the refresh of the display. On DX12 it presents with a sync
  interval of 0, like the Direct3D 11 renderer. Metal has no Mailbox and
  keeps Fifo.
- Startup log: `GPUI_STARTUP_LOG` logs the time of the startup stages of
  the renderers.
- Startup speed: the render pipelines build in parallel, wgpu does not
  validate indirect calls, Vulkan starts only when no DX12 adapter can
  draw to the window, and a resize does not wait for the GPU.
- Backends from the environment: `WGPU_BACKEND` and the wgpu backend
  options override the default, DX12 and then Vulkan on Windows. On
  Windows each backend gets an instance of its own, so no instance with
  Vulkan and DX12 drops after use. DX12 comes first: on a display of an
  NVIDIA GPU, a Vulkan window shows black frames while it moves.
- Device loss recovery: the wgpu window renderer of macOS and Windows makes
  a new device after a lost one, and asks the next frame to build the scene
  again. The renderer keeps its settings, such as the text contrast and the
  transparency, and its count of failed frames on the new device.
  `WgpuSurfaceFrame::device_generation` tells the new device from the lost
  one: wgpu can compare them as equal. `Window::gpu_failure` gives the GPU
  error of a wgpu window that stays after the renderer dropped its GPU
  caches to recover. The renderer keeps trying to recover and does not
  panic. The wgpu windows of a thread share one device. It drops with the
  last of them, so a thread can end after it closed its windows.
- Adapter by name: the wgpu renderer tries first the adapter whose name
  contains `WGPU_ADAPTER_NAME`, case-insensitive. It ranks above the
  adapter of `ZED_DEVICE_ID`. The first window panics when no adapter has
  the name. A recovery from a lost device with no such adapter fails, and
  the renderer counts it as a failed frame and tries again.
- Cached views: a cached view keeps its window control areas, such as
  the drag area of a title bar, in the frames that reuse its paint. Tests
  run the hit test of the window control areas with
  `VisualTestContext::hit_test_window_control_area`.
- Headless apps: the `headless` feature runs an app on the test platform
  in real time and draws each frame with the wgpu headless renderer.
  `Window::named_element_bounds` gives the visible bounds of each named
  element. The name of an element is its debug selector, or the id of an
  element that takes presses. `Window::render_to_image` works in every build: the wgpu
  renderer draws the scene to an offscreen texture and reads it back, also
  for X11 and Wayland windows.
- Decorations on the test platform: `VisualTestContext::simulate_decorations`
  makes a test window report client decorations, so a test can draw the
  window with a shadow inset by `Window::set_client_inset`.
- Resize on the test platform: `Window::resize` resizes a test window in a
  later task, through its resize callback, as a platform delivers a resize
  as a window event. The layout of the window then takes the size.
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
  and Wayland. On Wayland the tablet tools show it too, and the cursor
  theme loads once for each scale of the outputs under the pointer and the
  tools. A platform that cannot show the image shows the fallback style.
  The native cursor of an image lives as long as the image.
- Pen input on Windows: `Window::on_pen` reads the pointer messages of
  Windows Ink, with the whole history of each message, and the packets
  of a WinTab context. `App::set_pen_api` picks Windows Ink, WinTab, or
  WinTab until Windows Ink gives the first touch of a pen. A thread
  other than the UI thread opens each WinTab context, so a driver that
  does not answer blocks no window.
- Cancelled clicks: `Window::cancel_click` keeps the press of the mouse
  button that is down from firing click listeners at its release, as a
  drag of GPUI does. An app drag that does not use the drags of GPUI calls
  it once the press becomes a drag.
- Hover of a held mouse button: while a hitbox captures the pointer with
  `Window::capture_pointer`, as in the pointer capture of browsers, the
  hovered hitboxes are the captured one and the ones beneath it, from a hit
  test at the pointer moved into its bounds. So an ancestor keeps its hover
  style and cursor for the whole drag, and no other hitbox is hovered. No
  tooltip shows. During a capture the hover ignores the input modality. With
  no capture, a held button hovers only the hitboxes under both the pointer
  and the press, as the implicit grab of Qt. So the mouse up of a drag goes
  to no element under the pointer but not under the press, and an element
  that moves under the pointer during its drag captures the pointer to stay
  hovered. A drag and drop ends this rule, so its drop targets are hovered.
  Scroll events, `is_hovered_at` and the mouse up out of an element still
  test the hitboxes under the pointer. A view that draws again gets new
  hitbox ids, so a capture lasts one frame: an element captures the pointer
  in the prepaint of each frame of its drag. A call in an event listener
  captures it in the frame on screen, after the dispatch of the event. The
  release of the last held button ends both rules. `release_pointer` is
  gone.
- Quit on macOS: `Platform::quit` stops the AppKit run loop, and `run`
  calls the quit callback after it and returns, as on Linux and Windows.
  A quit that the system asks for, such as at logout, still ends the
  process with `exit` in `terminate:`.
- Wrapped rows fit: a row that is still too wide after a break at a
  break opportunity breaks again, so no row is wider than the wrap width
  when each glyph fits. The spaces at the end of a row hang past the edge,
  as in CSS. They get no row of their own and take no part in the alignment
  of the row.
- Break points of the caller: `StyledText::with_break_points` wraps a text
  only at the given `BreakPoint`s, in place of the Unicode line break
  rules. Each point has a rank, 0 the most preferred. A row wraps at the
  last point that fits of the lowest rank that it has, and at any glyph
  when it has no point. Each line of a text with line breaks counts its
  points from its own start. A cut text drops the points.
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
- Scroll to an item in its frame: `ScrollHandle::scroll_to_item` scrolls
  with the viewport bounds, the overflow and the child bounds of the frame
  that shows the scroll. A box that changes its size in that frame shows
  the whole item, and a request before the first frame scrolls too.
- Rendered focus handles: `FocusHandle::is_rendered` tells whether an
  element of the last rendered frame tracks the handle. A focus-lost
  listener moves the focus with no new frame, so it checks that the place
  it picks is drawn.
- Variable font weights on Linux and macOS: the text system rounds each
  requested weight to a whole number before a platform matches it, so near
  weights draw the same face on each platform. A variable face draws the
  weight clamped to its weight axis. A static face draws its own weight.
  Each weight that a face draws gets its own font id, so each cache keyed
  by the font id covers the weight. On Linux, a fallback face draws at the
  weight of its run. On macOS, a named instance of a variable font, such
  as a face of the system font, matches as a face of its own weight, and a
  copy of the nearest face draws each other weight. Each platform draws a
  variable font that the app adds at the default of its optical size axis
  at each size. On macOS, a system font keeps the optical size that Core
  Text picks for its size. `gpui::variable_font_conformance` holds the
  checks that the text system of each platform runs.
- Tooltips under their element: the tooltip of an element sits centered
  under the element, or above it when there is no room below, inside the
  frame of the window, and follows the element when it moves. The view of
  the tooltip holds its gap to the element as a margin. The tooltip of
  interactive text sits under the hovered line at the pointer.
  `Window::set_tooltip` keeps the place at the pointer. While a tooltip
  shows, and for 300 ms after its element lost the pointer, the next
  tooltip shows with no delay.
- Tooltip confines: `Window::confine_tooltips` limits the tooltips of a
  frame to areas, such as the cards of open popups. A tooltip shows only
  while the pointer is in one of them, so no tooltip of an element outside
  a popup covers it.
- Element opacity: `Window::element_opacity` gives the opacity that the
  current element draws with, in its prepaint and paint.
  `Window::with_element_opacity` multiplies the opacity of the elements
  that a closure draws, so an element can fade its child by the bounds
  that the child paints at.
- Outline: `Styled::outline` draws a line of a width and a color around
  the box of an element, with a gap of `offset`, as `outline` and
  `outline-offset` in CSS. A negative offset draws it inside the box. The
  line follows the round corners of the box, and draws over its content.
- Spring colors: `Interpolate` for `Rgba` and `Hsla` mixes with
  premultiplied alpha, as CSS does, so a clear color adds no tint on the
  way.
- Animating frames: `Window::animating` tells whether the last frame drew a
  one-shot animation or a moving spring before its end. A repeating
  animation and the repeating part of a chain do not count. A cached view
  that draws from its cache counts as it did when it last rendered.
- Springs per coordinate: a `SpringVector` target gives its coordinates as
  `SpringCoordinates`, and each coordinate moves on a spring of its own, so
  a position or a rectangle keeps a velocity for each component. Each
  `SpringTarget` is a vector of one coordinate. `SpringVector::tolerances`
  gives the settle tolerance of each coordinate, in place of the epsilon of
  the animation. `SpringConfig::critically_damped` makes a spring that comes
  within 1% of its target in a duration, or jumps to it with a zero
  duration.
- Window frame: `Window::frame_bounds` gives the part of the surface that the
  user sees as the window. A window with client decorations draws a shadow
  on each side that is not tiled, as `Tiling::shadow` and `Tiling::inset`
  state. The tooltips of the window stay inside the frame. The test window
  reports client decorations with `simulate_decorations`.
- Painted shadows: `Window::painted_shadows` gives the shadows of the last
  frame to tests, as `painted_quads` gives its quads.
- Debug bounds of any name: `Window::debug_bounds` in a headless app and
  `VisualTestContext::debug_bounds` take a name of any lifetime. The name is a
  debug selector, or the id of an element that takes presses.
- Shadow spread radius: the spread of a box shadow grows its corner radii
  as CSS does, so a ring around a round element stays round. An inset
  shadow and an outline shape their corners by the same rule.
- Shadow edges: a shadow starts at its element as drawn, at whole device
  pixels, and keeps the fraction of its offset and spread. The shader
  anti-aliases its edge, so a ring of 1.5 px draws as 1 solid and 1 half
  pixel, as in a browser.
- Tab groups that wrap: a tab group made with `tab_wrap` keeps
  `focus_next` and `focus_prev` inside it while the focus is in it, and
  wraps at its ends, so a modal view keeps the keys. A wrapping group
  inside another keeps the steps of the inner group. The focus of a tab
  step asks each scroll container around it to show the element and its
  outline, with the least scroll. `scroll_padding` keeps the element that
  far inside the edges of the container. A list passes a request from
  before it to the containers outside. `Window::tab_step_pending` tells the
  focus listeners of the draw that a tab step moved the focus. A tab step
  to the element that has the focus leaves no step pending.
- Drag threshold per element: `drag_threshold` sets how far the pointer
  moves with the button down before a drag of the element starts. The
  default, `DRAG_THRESHOLD`, is public.
- Letter spacing: `Styled::letter_spacing` and `TextStyle::letter_spacing`
  set the extra space between adjacent characters, in `Em` of the font
  size of each text. `WindowTextSystem::shape_spaced_line` shapes a line
  with such a spacing. A line has one spacing, so `TextRun` keeps its
  fields and the crates that build one by a struct literal still compile.
  The layout cache moves the shaped glyphs apart after the platform
  shaping, so it works the same on each platform. Grapheme clusters are
  the units: a mark stays with its base and a ligature is one unit. A line
  has no gap at its ends, and a unit with no advance gets no gap, as CSS
  Text 3 states.
  A spaced line takes its fonts with `liga`, `clig` and `calt` off, unless
  their features set them. A wrap or a truncation adds the spacing between
  the grapheme clusters of each row, as the shaped line does.
- Release after a test draw: in a test app, the flush that draws the dirty
  windows then releases the entities and the focus handles that the draw
  dropped, as the update of a platform frame does. A release observer of an
  element state that stops drawing runs before the flush ends.
- X11-only builds: the pipe helpers that only Wayland reads build only with
  the `wayland` feature, so a build with only `x11` has no dead code.
- Pointer lock: `Window::lock_pointer` holds the system cursor at the mouse
  position, as the pointer lock of browsers, and `Window::pointer_lock`
  gives that point. While locked, the mouse events and the mouse position
  report a virtual pointer that the relative motion of the mouse moves. A
  scroll or a pinch goes to the views at the lock point and reports that
  point. A call outside a draw starts a lock, and it fails in a window
  that is not active. A frame keeps the lock when it calls it in prepaint,
  so a lock lasts one frame. The release of the last held button ends it after its
  listeners, and so do a change of the active status or of the bounds.
  Then the mouse moves to the point of the lock. A platform that ends the
  lock by itself, such as a Wayland compositor that breaks it, tells the
  window through `PlatformWindow::on_pointer_unlock`. Windows clips the
  cursor to one pixel and reads raw input at the pointer speed of the
  system, as SDL. macOS moves the cursor to the point and disconnects the
  mouse from it. X11 grabs
  the pointer into a child window of one pixel and reads raw motion of
  XInput 2. The motion events of the window that the server sends before
  raw motion starts, such as the motion right after a press, move the
  virtual pointer to their position. Wayland locks the pointer with the pointer constraints protocol
  and reads the relative pointer protocol. A pen moves the virtual pointer
  by the motion of its tip, so the virtual pointer follows it, and a touch
  of the pen reports the virtual pointer. On Wayland the lock holds no pen
  cursor, and the pen events keep their positions.

## License

Apache-2.0, as the crates it holds. See `LICENSE-APACHE`.
