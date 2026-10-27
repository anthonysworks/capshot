//! Constant-rate frame clock.

use std::cell::Cell;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer,
    TIMER_ALL_ACCESS, WaitForSingleObject,
};
use windows::core::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    /// Time to emit frame `index`.
    Frame(u64),
    /// The previous frames took too long: `skipped` slots were dropped and
    /// the next frame to emit is `index`.
    Late { index: u64, skipped: u64 },
}

/// Wakes up at `start + index / fps` using a high-resolution waitable timer
/// (sub-millisecond accuracy, unlike `Sleep`, which is tied to the 1–15.6 ms
/// system tick).
pub struct Pacer {
    timer: HANDLE,
    start: Instant,
    period: Duration,
    next: Cell<u64>,
    skipped: Cell<u64>,
}

impl Pacer {
    pub fn new(fps: u32) -> Result<Self> {
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
            start: Instant::now(),
            period: Duration::from_secs(1) / fps.max(1),
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

    pub fn wait_next(&self) -> Tick {
        let index = self.next.get();
        let target = self.start + self.period * u32::try_from(index).unwrap_or(u32::MAX);
        let now = Instant::now();
        let tick = if now < target {
            self.sleep(target - now);
            Tick::Frame(index)
        } else {
            let behind = now - target;
            #[allow(clippy::cast_possible_truncation)]
            let skipped = (behind.as_nanos() / self.period.as_nanos()) as u64;
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
        let emitted = match tick {
            Tick::Frame(i) | Tick::Late { index: i, .. } => i,
        };
        self.next.set(emitted + 1);
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
    use super::*;

    #[test]
    fn keeps_a_steady_rate() {
        let pacer = Pacer::new(100).unwrap();
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
        let pacer = Pacer::new(100).unwrap();
        assert_eq!(pacer.wait_next(), Tick::Frame(0));
        std::thread::sleep(Duration::from_millis(45));
        match pacer.wait_next() {
            Tick::Late { index, skipped } => {
                assert!(skipped >= 3, "{skipped}");
                assert_eq!(index, 1 + skipped);
            }
            Tick::Frame(_) => panic!("expected a late tick"),
        }
        assert_eq!(pacer.skipped(), pacer.next.get() - 2);
    }
}
