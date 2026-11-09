//! Guided checks of a driver not yet verified on hardware: each check
//! changes one setting on the real mouse, asks the user what happened and
//! puts the setting back. Every change is undone on answer, Cancel or close.
//!
//! ```text
//!   DPI      set a clearly different value  -> "did the cursor speed change?" -> restore
//!   Rate     set 125 Hz, measure the input  -> "Done"                         -> restore
//!   Battery  show the value read            -> "does it match?"               (nothing set)
//! ```
//!
//! Checks whose current value is unknown are left out: what cannot be
//! read back cannot be restored.

pub use crate::view::Answer;

/// What the checks may change on the mouse (the mouse service).
pub trait Knobs {
    fn set_dpi(&mut self, dpi: u32);
    fn set_rate(&mut self, hz: u32);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Dpi,
    Rate,
    Battery,
}

/// Mouse state the checks start from (last reading and abilities).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Start {
    pub dpi: Option<u32>,
    /// DPI values the mouse accepts.
    pub values: Vec<u32>,
    pub rate: Option<u32>,
    /// Report rates the mouse accepts.
    pub rates: Vec<u32>,
    pub battery: Option<u8>,
}

/// Rate the rate check sets: low enough to feel and to measure.
const TEST_RATE: u32 = 125;
/// The test DPI is this many times lower (or higher) than the current one.
const DPI_FACTOR: u32 = 3;
/// From this DPI on the test value goes down, below it up.
const DPI_PIVOT: u32 = 1200;
/// A measured rate this close to the set one counts as a pass.
const RATE_TOLERANCE: u32 = 30;

pub struct Verify {
    checks: Vec<Check>,
    at: usize,
    start: Start,
    test_dpi: u32,
    test_rate: u32,
    /// A setting changed on the mouse and not yet put back.
    pending: Option<Check>,
    lines: Vec<String>,
}

/// Accepted value nearest to `want`.
fn nearest(values: &[u32], want: u32) -> Option<u32> {
    values.iter().copied().min_by_key(|v| v.abs_diff(want))
}

/// A DPI clearly different from `dpi`: 1600 -> ~533, 400 -> ~1200.
fn other_dpi(dpi: u32, values: &[u32]) -> Option<u32> {
    let want = if dpi >= DPI_PIVOT { dpi / DPI_FACTOR } else { dpi * DPI_FACTOR };
    nearest(values, want).filter(|&v| v != dpi)
}

/// 125 Hz, or the highest rate when the mouse already runs at 125 Hz.
fn other_rate(rate: u32, rates: &[u32]) -> Option<u32> {
    let want = if rate == TEST_RATE { rates.iter().copied().max()? } else { TEST_RATE };
    rates.contains(&want).then_some(want).filter(|&r| r != rate)
}

fn word(a: Answer) -> &'static str {
    match a {
        Answer::Yes => "yes",
        Answer::No => "no",
        Answer::Skip | Answer::Done => "skipped",
    }
}

impl Verify {
    /// The checks this mouse allows; `None` when there is nothing to check.
    pub fn new(start: Start) -> Option<Verify> {
        let test_dpi = start.dpi.and_then(|d| other_dpi(d, &start.values));
        let test_rate = start.rate.and_then(|r| other_rate(r, &start.rates));
        let mut checks = Vec::new();
        if test_dpi.is_some() {
            checks.push(Check::Dpi);
        }
        if test_rate.is_some() {
            checks.push(Check::Rate);
        }
        if start.battery.is_some() {
            checks.push(Check::Battery);
        }
        if checks.is_empty() {
            return None;
        }
        let (test_dpi, test_rate) = (test_dpi.unwrap_or_default(), test_rate.unwrap_or_default());
        Some(Verify { checks, at: 0, start, test_dpi, test_rate, pending: None, lines: Vec::new() })
    }

    /// The open check; `None` once all are answered or cancelled.
    pub fn current(&self) -> Option<Check> {
        self.checks.get(self.at).copied()
    }

    /// (step, count) for "Check 1 of 3".
    pub fn position(&self) -> (usize, usize) {
        (self.at + 1, self.checks.len())
    }

    pub fn test_dpi(&self) -> u32 {
        self.test_dpi
    }

    pub fn test_rate(&self) -> u32 {
        self.test_rate
    }

    pub fn start(&self) -> &Start {
        &self.start
    }

    /// Applies the open check's change to the mouse.
    pub fn begin(&mut self, k: &mut impl Knobs) {
        match self.current() {
            Some(Check::Dpi) => k.set_dpi(self.test_dpi),
            Some(Check::Rate) => k.set_rate(self.test_rate),
            Some(Check::Battery) | None => return,
        }
        self.pending = self.current();
    }

    /// Records the answer, puts the setting back and opens the next check.
    /// `measured` = report rate measured during the rate check.
    pub fn answer(&mut self, a: Answer, measured: Option<u32>, k: &mut impl Knobs) {
        let Some(check) = self.current() else {
            return;
        };
        self.restore(k);
        let line = self.result(check, a, measured);
        self.lines.push(line);
        self.at += 1;
        self.begin(k);
    }

    /// Puts back what is changed; the remaining checks count as cancelled.
    pub fn cancel(&mut self, k: &mut impl Knobs) {
        self.restore(k);
        while let Some(c) = self.current() {
            self.lines.push(format!("- {}: cancelled", title(c)));
            self.at += 1;
        }
    }

    /// Result lines for the report, one per check.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    fn restore(&mut self, k: &mut impl Knobs) {
        match self.pending.take() {
            Some(Check::Dpi) => k.set_dpi(self.start.dpi.unwrap_or_default()),
            Some(Check::Rate) => k.set_rate(self.start.rate.unwrap_or_default()),
            Some(Check::Battery) | None => {}
        }
    }

    fn result(&self, c: Check, a: Answer, measured: Option<u32>) -> String {
        let s = &self.start;
        match c {
            Check::Dpi => {
                let from = s.dpi.unwrap_or_default();
                format!("- DPI: set {from} -> {} DPI; cursor speed changed: {} (restored {from})", self.test_dpi, word(a))
            }
            Check::Rate => {
                let from = s.rate.unwrap_or_default();
                let got = match (a, measured) {
                    (Answer::Skip, _) => "skipped".to_string(),
                    (_, None) => "not measured".to_string(),
                    (_, Some(hz)) => {
                        let ok = if hz.abs_diff(self.test_rate) <= RATE_TOLERANCE { "pass" } else { "FAIL" };
                        format!("measured {hz} Hz, {ok}")
                    }
                };
                format!("- Report rate: set {from} -> {} Hz; {got} (restored {from} Hz)", self.test_rate)
            }
            Check::Battery => {
                let pct = s.battery.unwrap_or_default();
                format!("- Battery: Whisker reads {pct} %; matches the mouse or vendor app: {}", word(a))
            }
        }
    }
}

fn title(c: Check) -> &'static str {
    match c {
        Check::Dpi => "DPI",
        Check::Rate => "Report rate",
        Check::Battery => "Battery",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mouse settings as the fake mouse holds them.
    #[derive(Default)]
    struct Fake {
        dpi: u32,
        rate: u32,
        writes: usize,
    }

    impl Knobs for Fake {
        fn set_dpi(&mut self, dpi: u32) {
            (self.dpi, self.writes) = (dpi, self.writes + 1);
        }
        fn set_rate(&mut self, hz: u32) {
            (self.rate, self.writes) = (hz, self.writes + 1);
        }
    }

    fn start() -> Start {
        let values = (100..=16000).step_by(50).collect();
        Start { dpi: Some(1600), values, rate: Some(1000), rates: vec![125, 250, 500, 1000], battery: Some(76) }
    }

    fn mouse() -> Fake {
        Fake { dpi: 1600, rate: 1000, writes: 0 }
    }

    #[test]
    fn checks_follow_what_can_be_restored() {
        let v = Verify::new(start()).unwrap();
        assert_eq!(v.checks, vec![Check::Dpi, Check::Rate, Check::Battery]);
        assert_eq!(v.test_dpi(), 550);
        assert_eq!(v.test_rate(), 125);

        // DPI or rate not read back: never changed.
        let blind = Start { dpi: None, rate: None, ..start() };
        assert_eq!(Verify::new(blind).unwrap().checks, vec![Check::Battery]);
        let nothing = Start { dpi: None, rate: None, battery: None, ..start() };
        assert!(Verify::new(nothing).is_none());
        // Already at 125 Hz: the highest rate instead.
        assert_eq!(other_rate(125, &[125, 1000]), Some(1000));
        assert_eq!(other_dpi(400, &[400, 1200, 1600]), Some(1200));
    }

    #[test]
    fn full_run_restores_every_setting() {
        let (mut v, mut m) = (Verify::new(start()).unwrap(), mouse());
        v.begin(&mut m);
        assert_eq!(m.dpi, 550);
        v.answer(Answer::Yes, None, &mut m);
        assert_eq!((m.dpi, m.rate), (1600, 125));
        v.answer(Answer::Done, Some(124), &mut m);
        assert_eq!((m.dpi, m.rate), (1600, 1000));
        v.answer(Answer::No, None, &mut m);

        assert_eq!(v.current(), None);
        assert_eq!(m.writes, 4, "two changes, two restores");
        let text = v.lines().join("\n");
        assert!(text.contains("cursor speed changed: yes"), "{text}");
        assert!(text.contains("measured 124 Hz, pass"), "{text}");
        assert!(text.contains("76 %; matches the mouse or vendor app: no"), "{text}");
    }

    #[test]
    fn cancel_restores_the_changed_setting() {
        // Cancel during the DPI check.
        let (mut v, mut m) = (Verify::new(start()).unwrap(), mouse());
        v.begin(&mut m);
        v.cancel(&mut m);
        assert_eq!((m.dpi, m.rate), (1600, 1000));
        assert_eq!(v.current(), None);
        assert_eq!(v.lines().len(), 3);

        // Close during the rate check.
        let (mut v, mut m) = (Verify::new(start()).unwrap(), mouse());
        v.begin(&mut m);
        v.answer(Answer::Skip, None, &mut m);
        assert_eq!(m.rate, 125);
        v.cancel(&mut m);
        assert_eq!((m.dpi, m.rate), (1600, 1000));

        // A second cancel sends nothing more.
        let writes = m.writes;
        v.cancel(&mut m);
        assert_eq!(m.writes, writes);
    }

    #[test]
    fn far_off_rate_fails() {
        let (mut v, mut m) = (Verify::new(start()).unwrap(), mouse());
        v.begin(&mut m);
        v.answer(Answer::Skip, None, &mut m);
        v.answer(Answer::Done, Some(1000), &mut m);
        assert!(v.lines()[1].contains("measured 1000 Hz, FAIL"), "{:?}", v.lines());
    }
}
