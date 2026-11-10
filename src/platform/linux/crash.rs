//! Crash notes: a fatal signal is written into the log before the
//! process dies, e.g. `CRASH signal SIGSEGV`.

use crate::log::crash_raw;

/// Fatal signals and their log lines (`log::CRASH_MARK` + name). Fixed
/// bytes: a signal handler must not allocate.
const SIGNALS: [(libc::c_int, &[u8]); 5] = [
    (libc::SIGSEGV, b"        ? CRASH signal SIGSEGV\n"),
    (libc::SIGBUS, b"        ? CRASH signal SIGBUS\n"),
    (libc::SIGILL, b"        ? CRASH signal SIGILL\n"),
    (libc::SIGFPE, b"        ? CRASH signal SIGFPE\n"),
    (libc::SIGABRT, b"        ? CRASH signal SIGABRT\n"),
];

/// Logs fatal signals from now on.
pub fn install() {
    for (sig, _) in SIGNALS {
        // SAFETY: a zeroed sigaction is valid; the handler only writes.
        unsafe {
            let mut act: libc::sigaction = std::mem::zeroed();
            act.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as usize;
            // One shot: the default action then ends the process as usual.
            // On the alternate stack, so a stack overflow is logged too.
            act.sa_flags = libc::SA_RESETHAND | libc::SA_ONSTACK;
            libc::sigaction(sig, &act, std::ptr::null_mut());
        }
    }
}

extern "C" fn on_signal(sig: libc::c_int) {
    if let Some((_, line)) = SIGNALS.iter().find(|(s, _)| *s == sig) {
        crash_raw(line);
    }
    // Returning repeats the fault (or lets abort() go on) with the
    // default action restored.
}

#[cfg(test)]
mod tests {
    use super::SIGNALS;
    use crate::log::CRASH_MARK;

    #[test]
    fn lines_carry_the_crash_mark() {
        for (_, line) in SIGNALS {
            assert!(line.starts_with(CRASH_MARK.as_bytes()));
            assert!(line.ends_with(b"\n"));
        }
    }
}
