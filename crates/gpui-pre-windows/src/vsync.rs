use std::{
    sync::{
        Arc, Condvar, LazyLock, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use gpui::FrameRequestSource;
use gpui_util::ResultExt;
use windows::Win32::{
    Foundation::HWND,
    Graphics::Dwm::{DWM_TIMING_INFO, DwmFlush, DwmGetCompositionTimingInfo},
    System::Performance::QueryPerformanceFrequency,
};

static QPC_TICKS_PER_SECOND: LazyLock<u64> = LazyLock::new(|| {
    let mut frequency = 0;
    // On systems that run Windows XP or later, the function will always succeed and
    // will thus never return zero.
    unsafe { QueryPerformanceFrequency(&mut frequency).unwrap() };
    frequency as u64
});

const VSYNC_INTERVAL_THRESHOLD: Duration = Duration::from_millis(1);
const DEFAULT_VSYNC_INTERVAL: Duration = Duration::from_micros(16_666); // ~60Hz

pub(crate) struct VSyncProvider {
    interval: Duration,
    f: Box<dyn Fn() -> bool>,
}

impl VSyncProvider {
    pub(crate) fn new() -> Self {
        let interval = get_dwm_interval()
            .context("Failed to get DWM interval")
            .log_err()
            .unwrap_or(DEFAULT_VSYNC_INTERVAL);
        let f = Box::new(|| unsafe { DwmFlush().is_ok() });
        Self { interval, f }
    }

    pub(crate) fn wait_for_vsync(&self) -> FrameRequestSource {
        let vsync_start = Instant::now();
        let wait_succeeded = (self.f)();
        let elapsed = vsync_start.elapsed();
        // DwmFlush and DCompositionWaitForCompositorClock returns very early
        // instead of waiting until vblank when the monitor goes to sleep or is
        // unplugged (nothing to present due to desktop occlusion). We use 1ms as
        // a threshold for the duration of the wait functions and fallback to
        // Sleep() if it returns before that. This could happen during normal
        // operation for the first call after the vsync thread becomes non-idle,
        // but it shouldn't happen often.
        if !wait_succeeded || elapsed < VSYNC_INTERVAL_THRESHOLD {
            log::trace!("VSyncProvider::wait_for_vsync() took less time than expected");
            std::thread::sleep(self.interval);
            FrameRequestSource::LocalSchedule
        } else {
            FrameRequestSource::NativeCallback
        }
    }
}

/// Wakes the vsync thread when a window asks for a frame. The thread sleeps
/// while no window asks, so an idle app does not wake on each vsync.
#[derive(Default)]
pub(crate) struct FrameRequests {
    state: Mutex<FrameRequestsState>,
    changed: Condvar,
}

#[derive(Default)]
struct FrameRequestsState {
    asked: bool,
    stopped: bool,
}

impl FrameRequests {
    fn ask(&self) {
        self.update(|state| state.asked = true);
    }

    /// Ends the vsync thread at its next wake.
    pub(crate) fn stop(&self) {
        self.update(|state| state.stopped = true);
    }

    pub(crate) fn is_stopped(&self) -> bool {
        self.lock().stopped
    }

    /// Waits until a window asks for a frame. Returns `false` when the
    /// thread must stop.
    pub(crate) fn wait(&self) -> bool {
        let mut state = self
            .changed
            .wait_while(self.lock(), |state| !state.asked && !state.stopped)
            .unwrap_or_else(PoisonError::into_inner);
        state.asked = false;
        !state.stopped
    }

    fn update(&self, change: impl FnOnce(&mut FrameRequestsState)) {
        change(&mut self.lock());
        self.changed.notify_one();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FrameRequestsState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Whether a window asks the vsync thread for a frame at the next vsync.
pub(crate) struct WindowFrames {
    asked: AtomicBool,
    requests: Arc<FrameRequests>,
}

impl WindowFrames {
    pub(crate) fn new(requests: Arc<FrameRequests>) -> Self {
        Self {
            asked: AtomicBool::new(false),
            requests,
        }
    }

    pub(crate) fn ask(&self) {
        self.asked.store(true, Ordering::Release);
        self.requests.ask();
    }

    /// Whether the window asked for a frame since the last call.
    pub(crate) fn take(&self) -> bool {
        self.asked.swap(false, Ordering::AcqRel)
    }
}

fn get_dwm_interval() -> Result<Duration> {
    let mut timing_info = DWM_TIMING_INFO {
        cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32,
        ..Default::default()
    };
    unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut timing_info) }?;
    let interval = retrieve_duration(timing_info.qpcRefreshPeriod, *QPC_TICKS_PER_SECOND);
    // Check for interval values that are impossibly low. A 29 microsecond
    // interval was seen (from a qpcRefreshPeriod of 60).
    if interval < VSYNC_INTERVAL_THRESHOLD {
        Ok(retrieve_duration(
            timing_info.rateRefresh.uiDenominator as u64,
            timing_info.rateRefresh.uiNumerator as u64,
        ))
    } else {
        Ok(interval)
    }
}

#[inline]
fn retrieve_duration(counts: u64, ticks_per_second: u64) -> Duration {
    let ticks_per_microsecond = ticks_per_second / 1_000_000;
    Duration::from_micros(counts / ticks_per_microsecond)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_compositor_wait_reports_native_callback() {
        let provider = VSyncProvider {
            interval: Duration::ZERO,
            f: Box::new(|| {
                std::thread::sleep(VSYNC_INTERVAL_THRESHOLD * 2);
                true
            }),
        };

        assert_eq!(
            provider.wait_for_vsync(),
            FrameRequestSource::NativeCallback
        );
    }

    #[test]
    fn failed_compositor_wait_reports_local_schedule_even_after_threshold() {
        let provider = VSyncProvider {
            interval: Duration::ZERO,
            f: Box::new(|| {
                std::thread::sleep(VSYNC_INTERVAL_THRESHOLD * 2);
                false
            }),
        };

        assert_eq!(provider.wait_for_vsync(), FrameRequestSource::LocalSchedule);
    }

    #[test]
    fn a_window_gets_one_frame_for_each_ask() {
        let requests = Arc::new(FrameRequests::default());
        let frames = WindowFrames::new(requests.clone());
        assert!(!frames.take(), "a new window asks for no frame");
        frames.ask();
        frames.ask();
        assert!(requests.wait(), "an ask wakes the thread");
        assert!(frames.take(), "the window gets the frame it asked for");
        assert!(!frames.take(), "one frame serves all asks before it");
    }

    #[test]
    fn the_thread_sleeps_until_a_window_asks() {
        let requests = Arc::new(FrameRequests::default());
        let frames = Arc::new(WindowFrames::new(requests.clone()));
        let (woke_sender, woke) = std::sync::mpsc::channel();
        let thread = std::thread::spawn({
            let requests = requests.clone();
            move || {
                while requests.wait() {
                    woke_sender.send(()).unwrap();
                }
            }
        });
        assert!(
            woke.recv_timeout(Duration::from_millis(100)).is_err(),
            "no wake while no window asks"
        );
        frames.ask();
        woke.recv_timeout(Duration::from_secs(10))
            .expect("an ask wakes the thread");
        requests.stop();
        thread.join().unwrap();
        assert!(requests.is_stopped());
    }
}
