//! Diagnostic log on disk (`%APPDATA%\Whisker\log.txt`): what the app did
//! and when, so a bug report shows where it stalled or crashed. It stays
//! on this computer until the user copies it.
//!
//! Never written: serial numbers, file paths, Bluetooth names and
//! addresses (people name devices after themselves), key presses, clicks.
//!
//! ```text
//! --- start 2026-11-10 Whisker 0.9.0, Windows 11 26200
//!     0.012 mice: 2 in 38 ms
//!    12.400 act ChangeMouse
//!    17.900 HANG window not answering for 5 s
//!    23.100 RECOVERED after 10 s
//! --- exit
//! ```
//! Seconds since the start line. `CRASH`, `HANG` and `RECOVERED` are the
//! marks `ended_badly` looks for on the next start.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::Instant;

const FILE: &str = "log.txt";
/// The file is cut back to `KEEP` once it grows past `MAX`.
const MAX: u64 = 256 * 1024;
const KEEP: usize = 128 * 1024;
/// Most of the log handed out for a bug report.
const SHARE: usize = 64 * 1024;
/// Runs handed out: the one that went wrong and the current one.
const SHARE_RUNS: usize = 2;
/// Longest device string logged; firmware decides what it holds.
const NAME_MAX: usize = 64;
/// A panic waits this often, 1 ms apart, for a line being written.
const CRASH_TRIES: u32 = 50;

/// Linux: read and write for the owner only.
#[cfg(unix)]
const PRIVATE_MODE: u32 = 0o600;

const START: &str = "--- start";
const EXIT: &str = "--- exit";
const CRASH: &str = "CRASH";
const HANG: &str = "HANG";
const RECOVERED: &str = "RECOVERED";
/// Column of the time, e.g. `"   12.400 "`.
const TIME_WIDTH: usize = 9;

/// How the previous run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Last {
    Fine,
    /// It crashed, or was closed while the window did not answer.
    Crashed,
}

struct Sink {
    path: PathBuf,
    file: File,
    size: u64,
    /// Last line and how often it came again, so a dragged slider is one line.
    last: String,
    repeats: u32,
}

static SINK: Mutex<Option<Sink>> = Mutex::new(None);
static T0: OnceLock<Instant> = OnceLock::new();

/// `log!("mice: {} in {} ms", n, ms)`: one line with the time.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::log::line(&format!($($arg)*))
    };
}

/// Opens the log in `dir` and starts a run with `header` (version, OS).
/// Tells how the run before ended.
pub fn start(dir: Option<PathBuf>, header: &str) -> Last {
    T0.get_or_init(Instant::now);
    let Some(dir) = dir else {
        return Last::Fine;
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(FILE);

    // The old log: judge its last run, keep its tail.
    let old = std::fs::read(&path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let last = if ended_badly(&old) { Last::Crashed } else { Last::Fine };
    if old.len() > KEEP {
        let _ = std::fs::write(&path, tail(&old, KEEP));
    }

    let Some(mut sink) = open(&path) else {
        return last;
    };
    sink.put(&format!("{START} {header}\n"));
    *lock() = Some(sink);
    last
}

/// One line with the seconds since start.
pub fn line(text: &str) {
    if let Some(s) = lock().as_mut() {
        s.line(text);
    }
}

/// The window stopped answering.
pub fn hang(text: &str) {
    line(&format!("{HANG} {text}"));
}

/// The window answers again.
pub fn recovered(text: &str) {
    line(&format!("{RECOVERED} {text}"));
}

/// A crash, from a panic hook. Waits only briefly for a line another
/// thread is writing: the note must not hang the dying process.
pub fn crash(text: &str) {
    let line = format!("{:>TIME_WIDTH$.3} {CRASH} {}\n", seconds(), one_line(text));
    for _ in 0..CRASH_TRIES {
        if crash_raw(line.as_bytes()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// A crash, from an exception filter or signal handler: `line` is written
/// as it is (it must hold `CRASH_MARK` and end in a newline). No
/// allocation, no waiting: false when the log was busy.
pub fn crash_raw(line: &[u8]) -> bool {
    let guard = match SINK.try_lock() {
        Ok(g) => g,
        Err(TryLockError::Poisoned(e)) => e.into_inner(),
        Err(TryLockError::WouldBlock) => return false,
    };
    if let Some(s) = guard.as_ref() {
        let _ = (&s.file).write_all(line);
    }
    true
}

/// A name a device reports, fit for the log: short, on one line.
pub fn name(s: &str) -> String {
    one_line(s).chars().take(NAME_MAX).collect()
}

/// Start of a `crash_raw` line: the time column left empty, then the mark.
pub const CRASH_MARK: &str = "        ? CRASH ";

/// A clean end of the run.
pub fn exit() {
    if let Some(mut s) = lock().take() {
        s.flush_repeats();
        s.put(&format!("{EXIT}\n"));
    }
}

/// The last `n` lines of the log, oldest first: what the app did last.
pub fn last_lines(n: usize) -> Vec<String> {
    let text = text();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].iter().map(|l| l.to_string()).collect()
}

/// The log for a bug report: the last run and the current one, so the
/// user hands out what the notice is about, not weeks of history.
pub fn text() -> String {
    let guard = lock();
    let Some(s) = guard.as_ref() else {
        return String::new();
    };
    let all = std::fs::read(&s.path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    tail(last_runs(&all, SHARE_RUNS), SHARE).to_string()
}

fn lock() -> MutexGuard<'static, Option<Sink>> {
    SINK.lock().unwrap_or_else(|e| e.into_inner())
}

fn seconds() -> f64 {
    T0.get().map_or(0.0, |t| t.elapsed().as_secs_f64())
}

fn open(path: &Path) -> Option<Sink> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    // The owner's eyes only: it tells which devices were used and when.
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(PRIVATE_MODE);
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(PRIVATE_MODE));
    }
    let file = options.open(path).ok()?;
    let size = file.metadata().map_or(0, |m| m.len());
    Some(Sink { path: path.to_path_buf(), file, size, last: String::new(), repeats: 0 })
}

impl Sink {
    fn line(&mut self, text: &str) {
        let text = one_line(text);
        if text == self.last {
            self.repeats += 1;
            return;
        }
        self.flush_repeats();
        self.put(&format!("{:>TIME_WIDTH$.3} {text}\n", seconds()));
        self.last = text;
    }

    /// "… 12 more times" for the line that kept coming.
    fn flush_repeats(&mut self) {
        if self.repeats == 0 {
            return;
        }
        let n = std::mem::take(&mut self.repeats);
        self.put(&format!("{:>TIME_WIDTH$} {n} more times\n", ""));
    }

    fn put(&mut self, s: &str) {
        if self.file.write_all(s.as_bytes()).is_err() {
            return;
        }
        self.size += s.len() as u64;
        if self.size > MAX {
            self.shrink();
        }
    }

    /// Rewrites the file with its newest `KEEP` bytes.
    fn shrink(&mut self) {
        // Counted from zero either way: a file that cannot be rewritten
        // (held by a scanner) must not be re-read on every line.
        self.size = 0;
        let Ok(bytes) = std::fs::read(&self.path) else {
            return;
        };
        let all = String::from_utf8_lossy(&bytes);
        if std::fs::write(&self.path, tail(&all, KEEP)).is_err() {
            return;
        }
        if let Some(fresh) = open(&self.path) {
            (self.file, self.size) = (fresh.file, fresh.size);
        }
    }
}

/// Line breaks inside a message would forge log lines; direction marks
/// would reorder it on screen. Device names are what brings them in.
fn one_line(text: &str) -> String {
    /// Line and paragraph separators, direction marks and overrides.
    const UNSAFE: [std::ops::RangeInclusive<char>; 3] = ['\u{2028}'..='\u{202E}', '\u{2066}'..='\u{2069}', '\u{200E}'..='\u{200F}'];
    let bad = |c: char| c.is_control() || UNSAFE.iter().any(|r| r.contains(&c));
    text.chars().map(|c| if bad(c) { ' ' } else { c }).collect()
}

/// Where each run starts: a start mark at the beginning of a line (one
/// inside a device name is no start).
fn run_starts(log: &str) -> Vec<usize> {
    let mut at = 0;
    let mut out = Vec::new();
    for l in log.split_inclusive('\n') {
        if l.starts_with(START) {
            out.push(at);
        }
        at += l.len();
    }
    out
}

/// The last `n` runs of `log`.
fn last_runs(log: &str, n: usize) -> &str {
    let starts = run_starts(log);
    match starts.len().checked_sub(n) {
        Some(first) => &log[starts[first]..],
        None => log,
    }
}

/// The last whole lines of `text` that fit in `max` bytes.
fn tail(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut at = text.len() - max;
    while !text.is_char_boundary(at) {
        at += 1;
    }
    // Drop the line that was cut in half.
    match text[at..].find('\n') {
        Some(nl) => &text[at + nl + 1..],
        None => &text[at..],
    }
}

/// Whether the last run in `log` crashed, or ended (without the exit
/// line) while its window did not answer. A run that was merely killed,
/// e.g. by a shutdown, is fine.
fn ended_badly(log: &str) -> bool {
    let run = last_runs(log, 1);
    let mark = |l: &str, m: &str| l.split_whitespace().take(2).any(|w| w == m);

    let mut hung = false;
    let mut exited = false;
    for l in run.lines() {
        if mark(l, CRASH) {
            return true;
        }
        if mark(l, HANG) {
            hung = true;
        } else if mark(l, RECOVERED) {
            hung = false;
        }
        exited = l.starts_with(EXIT);
    }
    hung && !exited
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "--- start 2026-11-10 Whisker 0.9.0, Windows 11 26200\n";

    #[test]
    fn clean_and_killed_runs_are_fine() {
        assert!(!ended_badly(""));
        assert!(!ended_badly(&format!("{HEAD}    0.012 mice: 2 in 38 ms\n--- exit\n")));
        // Killed by a shutdown: no exit line, nothing wrong either.
        assert!(!ended_badly(&format!("{HEAD}    0.012 mice: 2 in 38 ms\n")));
    }

    #[test]
    fn crash_is_bad_however_it_was_written() {
        assert!(ended_badly(&format!("{HEAD}    3.100 CRASH panic at src/app.rs:10: boom\n")));
        assert!(ended_badly(&format!("{HEAD}{CRASH_MARK}exception C0000005\n")));
        // A mouse named CRASH is no crash.
        assert!(!ended_badly(&format!("{HEAD}    0.500 mouse \"CRASH\" opened\n--- exit\n")));
    }

    #[test]
    fn hang_is_bad_only_when_the_run_died_in_it() {
        let hang = format!("{HEAD}   17.900 HANG window not answering for 5 s\n");
        assert!(ended_badly(&hang));
        assert!(!ended_badly(&format!("{hang}   23.100 RECOVERED after 10 s\n")));
        // Slow but clean exit.
        assert!(!ended_badly(&format!("{hang}--- exit\n")));
    }

    #[test]
    fn only_the_last_run_counts() {
        let crashed = format!("{HEAD}    3.100 CRASH panic\n");
        assert!(!ended_badly(&format!("{crashed}{HEAD}--- exit\n")));
        assert!(ended_badly(&format!("{HEAD}--- exit\n{crashed}")));
    }

    #[test]
    fn tail_keeps_whole_lines() {
        let text = "first line\nsecond line\nthird\n";
        assert_eq!(tail(text, 100), text);
        assert_eq!(tail(text, 10), "third\n");
        assert_eq!(tail(text, 20), "second line\nthird\n");
        // Never splits a character.
        assert_eq!(tail("ááá\nžž\n", 7), "žž\n");
    }

    #[test]
    fn messages_stay_on_one_line() {
        assert_eq!(one_line("name\r\n--- exit"), "name  --- exit");
        // Unicode line break, right-to-left override, escape, NUL.
        assert_eq!(one_line("a\u{2028}b\u{202E}c\u{1b}d\0e"), "a b c d e");
        assert_eq!(name(&"x".repeat(200)).len(), NAME_MAX);
    }

    #[test]
    fn a_device_name_cannot_fake_a_new_run() {
        // The name is logged after the crash; it must not hide it.
        let log = format!("{HEAD}    3.100 CRASH panic\n    3.200 mouse: \"--- start\"\n");
        assert!(ended_badly(&log));
    }

    #[test]
    fn only_the_last_two_runs_are_shared() {
        let log = format!("{HEAD}    1.000 old\n--- exit\n{HEAD}    2.000 bad\n{HEAD}    3.000 now\n");
        let shared = last_runs(&log, SHARE_RUNS);
        assert!(!shared.contains("old"));
        assert!(shared.contains("bad") && shared.contains("now"));
        // Fewer runs than asked for: everything.
        assert_eq!(last_runs("no start line\n", SHARE_RUNS), "no start line\n");
    }

    #[test]
    fn repeated_lines_fold_into_one() {
        // Its own sink: other tests may log through the shared one.
        let dir = std::env::temp_dir().join(format!("whisker-log-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(FILE);
        let _ = std::fs::remove_file(&path);

        let mut s = open(&path).unwrap();
        for _ in 0..4 {
            s.line("act Slider");
        }
        s.line("act Nav");
        drop(s);
        let got = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(got.matches("act Slider").count(), 1);
        assert!(got.contains("3 more times"));
        assert!(got.ends_with("act Nav
"));
    }
}
