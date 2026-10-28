//! Recording time: performance-counter time minus the time spent paused.
//!
//! Video and audio both read this clock, so a pause cuts both tracks at
//! exactly the same point and they stay in sync afterwards.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Times are in 100 ns units of the performance counter
/// (see [`crate::audio::now_ticks`]).
#[derive(Debug, Default)]
struct State {
    /// Set once capture and encoding are ready.
    origin: Option<i64>,
    paused: bool,
    paused_since: Option<i64>,
    /// Finished pauses, in absolute time.
    pauses: Vec<(i64, i64)>,
    paused_total: i64,
}

#[derive(Debug, Default)]
pub struct Clock {
    state: Mutex<State>,
}

impl Clock {
    /// A clock that has not started yet, optionally paused from the start
    /// (e.g. during a countdown).
    pub fn new(paused: bool) -> Self {
        Self {
            state: Mutex::new(State {
                paused,
                ..State::default()
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts the clock at absolute time `now`.
    pub fn begin(&self, now: i64) {
        let mut s = self.state();
        s.origin = Some(now);
        if s.paused {
            s.paused_since = Some(now);
        }
    }

    pub fn pause(&self, now: i64) {
        let mut s = self.state();
        if s.paused {
            return;
        }
        s.paused = true;
        if s.origin.is_some() {
            s.paused_since = Some(now);
        }
    }

    pub fn resume(&self, now: i64) {
        let mut s = self.state();
        s.paused = false;
        if let Some(since) = s.paused_since.take() {
            s.pauses.push((since, now));
            s.paused_total += now - since;
        }
    }

    pub fn is_paused(&self) -> bool {
        self.state().paused
    }

    /// Recording time at absolute time `now` (frozen while paused, 0 before
    /// the start).
    pub fn elapsed(&self, now: i64) -> i64 {
        let s = self.state();
        let Some(origin) = s.origin else {
            return 0;
        };
        let end = s.paused_since.unwrap_or(now);
        (end - origin - s.paused_total).max(0)
    }

    /// Maps an absolute timestamp (e.g. of an audio packet) to recording
    /// time, or `None` if it falls before the start or inside a pause.
    pub fn map(&self, at: i64) -> Option<i64> {
        let s = self.state();
        let origin = s.origin?;
        if at < origin || s.paused_since.is_some_and(|since| at >= since) {
            return None;
        }
        let mut skipped = 0;
        for &(start, end) in &s.pauses {
            if at >= end {
                skipped += end - start;
            } else if at >= start {
                return None;
            }
        }
        Some(at - origin - skipped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_from_the_origin() {
        let clock = Clock::new(false);
        assert_eq!(clock.elapsed(500), 0);
        assert_eq!(clock.map(500), None);
        clock.begin(1000);
        assert_eq!(clock.elapsed(1500), 500);
        assert_eq!(clock.map(1200), Some(200));
        assert_eq!(clock.map(900), None);
    }

    #[test]
    fn pauses_are_removed_from_the_timeline() {
        let clock = Clock::new(false);
        clock.begin(0);
        clock.pause(100);
        assert!(clock.is_paused());
        assert_eq!(clock.elapsed(150), 100);
        assert_eq!(clock.map(120), None);
        clock.resume(300);
        assert_eq!(clock.elapsed(400), 200);
        assert_eq!(clock.map(350), Some(150));
        assert_eq!(clock.map(50), Some(50));
        assert_eq!(clock.map(200), None);

        clock.pause(500);
        clock.resume(600);
        assert_eq!(clock.elapsed(700), 400);
        assert_eq!(clock.map(650), Some(350));
        assert_eq!(clock.map(550), None);
    }

    #[test]
    fn can_start_paused() {
        let clock = Clock::new(true);
        clock.begin(1000);
        assert!(clock.is_paused());
        assert_eq!(clock.elapsed(5000), 0);
        assert_eq!(clock.map(2000), None);
        clock.resume(4000);
        assert_eq!(clock.elapsed(4500), 500);
        assert_eq!(clock.map(4100), Some(100));
    }

    #[test]
    fn pause_before_begin_and_repeated_calls() {
        let clock = Clock::new(false);
        clock.pause(10);
        clock.pause(20);
        clock.begin(100);
        assert_eq!(clock.elapsed(200), 0);
        clock.resume(300);
        clock.resume(310);
        assert_eq!(clock.elapsed(400), 100);
    }
}
