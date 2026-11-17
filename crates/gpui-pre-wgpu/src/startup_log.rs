//! `GPUI_STARTUP_LOG=1` prints the time of each startup stage of the renderers to stderr.
//! A line is `proto startup-<stage> <ms> <end>`. `<end>` is the Unix time in ms at the end
//! of the stage, so a launcher can put the stages on the timeline of the process.
//! On each platform, the stage `window-renderer` spans the start of the renderer of a
//! window, and holds the other stages of that start.

use std::sync::LazyLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn startup_log_enabled() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var_os("GPUI_STARTUP_LOG").is_some());
    *ON
}

/// Runs `work` and logs its time as `stage`. It reads no clock while the log is off.
pub fn time_startup_stage<T>(stage: &str, work: impl FnOnce() -> T) -> T {
    if !startup_log_enabled() {
        return work();
    }
    let start = Instant::now();
    let result = work();
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let end = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64() * 1000.0);
    eprintln!("proto startup-{stage} {ms:.3} {end:.1}");
    result
}
