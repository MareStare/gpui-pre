//! `GPUI_FRAME_LOG=1` prints the time of each draw of a window renderer and the gap
//! between draws to stderr, as `proto frame-draw <ms>` and `proto frame-gap <ms>`.

use std::cell::Cell;
use std::sync::LazyLock;
use std::time::Instant;

use crate::StartupStage;

fn frame_log_enabled() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var_os("GPUI_FRAME_LOG").is_some());
    *ON
}

thread_local! {
    static FIRST_DRAW: Cell<bool> = const { Cell::new(true) };
    static LAST_DRAW: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Runs the draw of a window renderer. Logs the first draw of the thread as the
/// startup stage `first-draw`, and each draw to the frame log.
pub fn time_frame_draw<T>(draw: impl FnOnce() -> T) -> T {
    let first_stage = FIRST_DRAW.replace(false).then(StartupStage::start);
    let start = Instant::now();
    let result = draw();
    if let Some(stage) = first_stage {
        stage.end("first-draw");
    }
    if frame_log_enabled() {
        if let Some(last) = LAST_DRAW.replace(Some(start)) {
            eprintln!(
                "proto frame-gap {:.3}",
                (start - last).as_secs_f64() * 1000.0
            );
        }
        eprintln!(
            "proto frame-draw {:.3}",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
    result
}
