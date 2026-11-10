//! Constant-rate frame clock.

use std::cell::Cell;
use std::sync::Arc;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer,
    TIMER_ALL_ACCESS, WaitForSingleObject,
};
use windows::core::Result;

use crate::audio::now_ticks;
use crate::clock::Clock;
use crate::encoder::TICKS_PER_SECOND;

/// How often a paused recording checks whether it has resumed.
const PAUSED_POLL: Duration = Duration::from_millis(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    /// Time to emit frame `index`.
    Frame(u64),
    /// The previous frames took too long: `skipped` slots were dropped and
    /// the next frame to emit is `index`.
    Late { index: u64, skipped: u64 },
    /// The recording is paused: nothing to emit.
    Paused,
}

/// Wakes up when the recording clock reaches `index / fps`, using a
/// high-resolution waitable timer (sub-millisecond accuracy, unlike `Sleep`,
/// which is tied to the 1–15.6 ms system tick). Following the recording
/// clock makes pauses transparent: frame numbers simply continue.
pub struct Pacer {
    timer: HANDLE,
    clock: Arc<Clock>,
    period: i64,
    next: Cell<u64>,
    skipped: Cell<u64>,
}

impl Pacer {
    pub fn new(fps: u32, clock: Arc<Clock>) -> Result<Self> {
        // SAFETY: creates an unnamed timer owned by this object.
        let timer = unsafe {
            CreateWaitableTimerExW(
                None,
                None,
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )?
        };
        Ok(Self {
            timer,
            clock,
            period: TICKS_PER_SECOND / i64::from(fps.max(1)),
            next: Cell::new(0),
            skipped: Cell::new(0),
        })
    }

    /// Total frame slots skipped so far.
    pub fn skipped(&self) -> u64 {
        self.skipped.get()
    }

    fn sleep(&self, duration: Duration) {
        // Negative due time: relative, in 100 ns units.
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let due = -((duration.as_nanos() / 100) as i64).max(1);
        // SAFETY: the timer handle is valid for the lifetime of `self`.
        unsafe {
            if SetWaitableTimer(self.timer, &due, 0, None, None, false).is_ok() {
                WaitForSingleObject(self.timer, INFINITE);
            } else {
                std::thread::sleep(duration);
            }
        }
    }

    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    pub fn wait_next(&self) -> Tick {
        if self.clock.is_paused() {
            self.sleep(PAUSED_POLL);
            return Tick::Paused;
        }
        let index = self.next.get();
        let target = index as i64 * self.period;
        let now = self.clock.elapsed(now_ticks());
        let tick = if now < target {
            self.sleep(Duration::from_nanos((target - now) as u64 * 100));
            Tick::Frame(index)
        } else {
            let skipped = ((now - target) / self.period) as u64;
            if skipped == 0 {
                Tick::Frame(index)
            } else {
                self.skipped.set(self.skipped.get() + skipped);
                Tick::Late {
                    index: index + skipped,
                    skipped,
                }
            }
        };
        if let Tick::Frame(i) | Tick::Late { index: i, .. } = tick {
            self.next.set(i + 1);
        }
        tick
    }
}

impl Drop for Pacer {
    fn drop(&mut self) {
        // SAFETY: closes the handle created in `new`.
        let _ = unsafe { CloseHandle(self.timer) };
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn running_clock() -> Arc<Clock> {
        let clock = Arc::new(Clock::new(false));
        clock.begin(now_ticks());
        clock
    }

    #[test]
    fn keeps_a_steady_rate() {
        let pacer = Pacer::new(100, running_clock()).unwrap();
        let start = Instant::now();
        for expected in 0..20 {
            assert_eq!(pacer.wait_next(), Tick::Frame(expected));
        }
        // 20 frames at 100 FPS: frame 19 is due at 190 ms.
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(185), "{elapsed:?}");
        assert!(elapsed < Duration::from_millis(260), "{elapsed:?}");
    }

    #[test]
    fn skips_slots_when_late() {
        let pacer = Pacer::new(100, running_clock()).unwrap();
        assert_eq!(pacer.wait_next(), Tick::Frame(0));
        std::thread::sleep(Duration::from_millis(45));
        match pacer.wait_next() {
            Tick::Late { index, skipped } => {
                assert!(skipped >= 3, "{skipped}");
                assert_eq!(index, 1 + skipped);
            }
            other => panic!("expected a late tick, got {other:?}"),
        }
        assert_eq!(pacer.skipped(), pacer.next.get() - 2);
    }

    #[test]
    fn pause_freezes_frame_numbers() {
        let clock = running_clock();
        let pacer = Pacer::new(100, Arc::clone(&clock)).unwrap();
        assert_eq!(pacer.wait_next(), Tick::Frame(0));
        clock.pause(now_ticks());
        assert_eq!(pacer.wait_next(), Tick::Paused);
        std::thread::sleep(Duration::from_millis(60));
        clock.resume(now_ticks());
        // The 60 ms pause is not counted: no slot is skipped.
        assert_eq!(pacer.wait_next(), Tick::Frame(1));
        assert_eq!(pacer.skipped(), 0);
    }
}
