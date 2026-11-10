//! Watches that the window still answers. A thread asks the UI thread
//! once a second; five unanswered seconds go into the log as a hang,
//! the answer after it as a recovery.
//!
//! ```text
//! watch thread:  ask ─ 1 s ─ answered? ─ yes → ask again
//!                                      └ no  → silent += 1; 5 → HANG
//! ```
//! Seconds are counted in wake-ups, not on the clock, so a computer
//! that slept is no hang.

use crate::platform::window;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

const TICK: Duration = Duration::from_secs(1);
/// Unanswered ticks that make a hang.
const LIMIT: u32 = 5;

static ANSWERED: AtomicU64 = AtomicU64::new(0);
static STOP: AtomicBool = AtomicBool::new(false);

#[derive(Debug, PartialEq, Eq)]
enum Note {
    Hang(u32),
    Recovered(u32),
}

/// Unanswered ticks so far.
#[derive(Default)]
struct Watch {
    silent: u32,
}

impl Watch {
    /// One tick: was the last question answered?
    fn tick(&mut self, answered: bool) -> Option<Note> {
        if answered {
            let was = std::mem::take(&mut self.silent);
            return (was >= LIMIT).then_some(Note::Recovered(was));
        }
        self.silent += 1;
        (self.silent == LIMIT).then_some(Note::Hang(LIMIT))
    }
}

/// Starts the watch thread; the window must exist.
pub fn start() {
    std::thread::spawn(|| {
        let mut watch = Watch::default();
        let mut asked = 0u64;

        while !STOP.load(Ordering::Relaxed) {
            let answered = ANSWERED.load(Ordering::Relaxed) == asked;
            match watch.tick(answered) {
                Some(Note::Hang(s)) => crate::log::hang(&format!("window not answering for {s} s")),
                Some(Note::Recovered(s)) => crate::log::recovered(&format!("after {s} s")),
                None => {}
            }
            // One question at a time: a stuck window gets no queue of them.
            if answered {
                asked += 1;
                let n = asked;
                window::post(move || ANSWERED.store(n, Ordering::Relaxed));
            }
            std::thread::sleep(TICK);
        }
    });
}

/// The event loop is over: silence from now on is no hang.
pub fn stop() {
    STOP.store(true, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_silent_seconds_are_one_hang() {
        let mut w = Watch::default();
        assert_eq!(w.tick(true), None);
        for _ in 1..LIMIT {
            assert_eq!(w.tick(false), None);
        }
        assert_eq!(w.tick(false), Some(Note::Hang(LIMIT)));
        // Still stuck: said once.
        assert_eq!(w.tick(false), None);
        assert_eq!(w.tick(false), None);
        assert_eq!(w.tick(true), Some(Note::Recovered(LIMIT + 2)));
        assert_eq!(w.tick(true), None);
    }

    #[test]
    fn a_short_stall_is_nothing() {
        let mut w = Watch::default();
        assert_eq!(w.tick(false), None);
        assert_eq!(w.tick(false), None);
        assert_eq!(w.tick(true), None);
    }
}
