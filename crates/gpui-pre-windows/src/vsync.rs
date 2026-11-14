use std::{
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use gpui::FrameRequestSource;
use gpui_util::ResultExt;
use windows::{
    Win32::{
        Foundation::{HANDLE, HWND, WAIT_OBJECT_0},
        Graphics::Dwm::{DWM_TIMING_INFO, DwmFlush, DwmGetCompositionTimingInfo},
        System::{
            Performance::QueryPerformanceFrequency,
            Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects},
        },
    },
    core::Owned,
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

/// The requests to the vsync thread. The thread sleeps until one comes, so an
/// idle app does not wake at each vsync.
pub(crate) struct VSyncRequests {
    /// An auto-reset event. Each request sets it.
    wake: Owned<HANDLE>,
    stopped: AtomicBool,
    devices_invalid: AtomicBool,
}

// SAFETY: Any thread can set and wait on an event handle.
unsafe impl Send for VSyncRequests {}
// SAFETY: As above.
unsafe impl Sync for VSyncRequests {}

/// Why the vsync thread woke.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum VSyncWake {
    /// A window may have asked for a frame.
    Frame,
    /// The DirectX devices must be made again.
    DevicesLost,
    Stop,
}

impl VSyncRequests {
    pub(crate) fn new() -> Result<Self> {
        // SAFETY: The handle is new and nothing else owns it.
        let wake = unsafe { Owned::new(CreateEventW(None, false, false, None)?) };
        Ok(Self {
            wake,
            stopped: AtomicBool::new(false),
            devices_invalid: AtomicBool::new(false),
        })
    }

    fn ask(&self) {
        // SAFETY: The handle is a valid event while `self` lives.
        unsafe { SetEvent(*self.wake) }.log_err();
    }

    /// Ends the vsync thread at its next wake.
    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.ask();
    }

    /// Makes the vsync thread make the DirectX devices again.
    pub(crate) fn invalidate_devices(&self) {
        self.devices_invalid.store(true, Ordering::Release);
        self.ask();
    }

    /// Waits for a request, or until `device_removed` is signaled.
    pub(crate) fn wait(&self, device_removed: Option<HANDLE>) -> VSyncWake {
        // The stop signal wakes one wait only. Later waits must not block.
        if self.stopped.load(Ordering::Acquire) {
            return VSyncWake::Stop;
        }
        // The device event comes first: when both are signaled, the wait
        // returns the first one, and the frame request stays set.
        let handles: &[HANDLE] = match &device_removed {
            Some(device_removed) => &[*device_removed, *self.wake],
            None => &[*self.wake],
        };
        // SAFETY: The handles are valid events while this call runs.
        let signaled = unsafe { WaitForMultipleObjects(handles, false, INFINITE) };
        if self.stopped.load(Ordering::Acquire) {
            VSyncWake::Stop
        } else if (device_removed.is_some() && signaled == WAIT_OBJECT_0)
            || self.devices_invalid.swap(false, Ordering::AcqRel)
        {
            VSyncWake::DevicesLost
        } else {
            VSyncWake::Frame
        }
    }
}

/// Whether a window asks the vsync thread for a frame at the next vsync.
pub(crate) struct WindowFrames {
    asked: AtomicBool,
    requests: Arc<VSyncRequests>,
}

impl WindowFrames {
    pub(crate) fn new(requests: Arc<VSyncRequests>) -> Self {
        Self {
            asked: AtomicBool::new(false),
            requests,
        }
    }

    pub(crate) fn ask(&self) {
        // The vsync thread takes the flag after it wakes, so only the first
        // ask after a take must wake it.
        if !self.asked.swap(true, Ordering::AcqRel) {
            self.requests.ask();
        }
    }

    /// Whether the window asked for a frame since the last call.
    pub(crate) fn take(&self) -> bool {
        self.asked.swap(false, Ordering::AcqRel)
    }

    pub(crate) fn requests(&self) -> &VSyncRequests {
        &self.requests
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
        let requests = Arc::new(VSyncRequests::new().unwrap());
        let frames = WindowFrames::new(requests.clone());
        assert!(!frames.take(), "a new window asks for no frame");
        frames.ask();
        frames.ask();
        assert_eq!(requests.wait(None), VSyncWake::Frame);
        assert!(frames.take(), "the window gets the frame it asked for");
        assert!(!frames.take(), "one frame serves all asks before it");
    }

    #[test]
    fn only_the_first_ask_before_a_take_wakes_the_thread() {
        let requests = Arc::new(VSyncRequests::new().unwrap());
        let frames = WindowFrames::new(requests.clone());
        frames.ask();
        assert_eq!(requests.wait(None), VSyncWake::Frame);
        frames.ask();
        assert!(
            !is_signaled(*requests.wake),
            "the window asked before the thread took its flag"
        );
        assert!(frames.take());
        frames.ask();
        assert!(
            is_signaled(*requests.wake),
            "an ask after a take wakes the thread"
        );
    }

    #[test]
    fn the_thread_sleeps_until_a_window_asks() {
        let requests = Arc::new(VSyncRequests::new().unwrap());
        let frames = Arc::new(WindowFrames::new(requests.clone()));
        let (woke_sender, woke) = std::sync::mpsc::channel();
        let thread = std::thread::spawn({
            let requests = requests.clone();
            move || {
                while requests.wait(None) == VSyncWake::Frame {
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
        assert_eq!(requests.wait(None), VSyncWake::Stop);
    }

    #[test]
    fn a_device_loss_wakes_the_thread_before_a_frame() {
        let requests = VSyncRequests::new().unwrap();
        // SAFETY: The handle is new and nothing else owns it.
        let device_removed = unsafe { Owned::new(CreateEventW(None, true, false, None).unwrap()) };
        requests.ask();
        // SAFETY: The handle is a valid event.
        unsafe { SetEvent(*device_removed) }.unwrap();
        assert_eq!(requests.wait(Some(*device_removed)), VSyncWake::DevicesLost);
        assert_eq!(
            requests.wait(None),
            VSyncWake::Frame,
            "the frame request stays set"
        );
    }

    #[test]
    fn invalid_devices_wake_the_thread() {
        let requests = VSyncRequests::new().unwrap();
        requests.invalidate_devices();
        assert_eq!(requests.wait(None), VSyncWake::DevicesLost);
        requests.ask();
        assert_eq!(
            requests.wait(None),
            VSyncWake::Frame,
            "the devices are made once"
        );
    }

    /// Whether an auto-reset event is set. Leaves it as it was.
    fn is_signaled(event: HANDLE) -> bool {
        use windows::Win32::System::Threading::WaitForSingleObject;
        // SAFETY: The handle is a valid event.
        let signaled = unsafe { WaitForSingleObject(event, 0) } == WAIT_OBJECT_0;
        if signaled {
            // The wait reset the event.
            // SAFETY: As above.
            unsafe { SetEvent(event) }.unwrap();
        }
        signaled
    }
}
