//! One running instance. An exclusive lock on a file in the per-user
//! runtime folder marks the running instance (the OS drops it when the
//! process ends, also on a crash). A second launch tells the first to
//! show its window, the uninstaller tells it to quit, both over a Unix
//! socket next to the lock (the folder is private to the user).
//!
//! ```text
//!   $XDG_RUNTIME_DIR/whisker.lock   flock held by the running instance
//!   $XDG_RUNTIME_DIR/whisker.sock   one byte per request: SHOW / QUIT
//! ```

use super::sys::runtime_dir;
use crate::platform::{Claim, Instance};
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const LOCK_NAME: &str = "whisker.lock";
const SOCKET_NAME: &str = "whisker.sock";
const SHOW: u8 = b's';
const QUIT: u8 = b'q';
const RETRY_GAP: Duration = Duration::from_millis(100);
const WAIT_LIMIT: Duration = Duration::from_secs(5);
const READ_LIMIT: Duration = Duration::from_secs(1);

/// Held for the life of the process.
static LOCK: OnceLock<File> = OnceLock::new();

fn lock_path() -> Option<PathBuf> {
    Some(runtime_dir()?.join(LOCK_NAME))
}

fn socket_path() -> Option<PathBuf> {
    Some(runtime_dir()?.join(SOCKET_NAME))
}

/// Opens the lock file and takes the exclusive lock if free.
fn try_lock() -> Option<File> {
    let path = lock_path()?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let f = File::options().create(true).truncate(false).write(true).open(path).ok()?;
    f.try_lock().ok()?;
    Some(f)
}

fn signal(request: u8) {
    let Some(mut s) = socket_path().and_then(|p| UnixStream::connect(p).ok()) else {
        return;
    };
    let _ = s.write_all(&[request]);
}

/// Claims the instance; on `AlreadyRunning` the first instance was told to show.
pub fn claim(mode: Claim) -> Instance {
    let start = Instant::now();
    loop {
        if let Some(f) = try_lock() {
            let _ = LOCK.set(f);
            return Instance::First;
        }
        if mode == Claim::Now || start.elapsed() > WAIT_LIMIT {
            break;
        }
        std::thread::sleep(RETRY_GAP);
    }

    signal(SHOW);
    Instance::AlreadyRunning
}

/// Asks a running instance to exit (used by the uninstaller).
pub fn request_quit() {
    signal(QUIT);
}

/// Waits until no instance holds the lock, at most `limit`.
pub fn wait_closed(limit: Duration) {
    let start = Instant::now();
    while start.elapsed() < limit {
        // Taken and dropped at once: nobody else holds it.
        if try_lock().is_some() {
            return;
        }
        std::thread::sleep(RETRY_GAP);
    }
}

/// Calls `on_show` / `on_quit` when another process asks for it.
pub fn listen(on_show: impl Fn() + Send + 'static, on_quit: impl Fn() + Send + 'static) {
    let Some(path) = socket_path() else {
        return;
    };
    // We hold the lock: a socket file left behind is stale.
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else {
        return;
    };
    std::thread::Builder::new()
        .name("single".into())
        .spawn(move || {
            for conn in listener.incoming() {
                // A lasting error (no file descriptors left) must not spin a core.
                let Ok(mut s) = conn else {
                    std::thread::sleep(RETRY_GAP);
                    continue;
                };
                // A client that never writes must not block the next one.
                let _ = s.set_read_timeout(Some(READ_LIMIT));
                let mut b = [0u8; 1];
                if s.read_exact(&mut b).is_err() {
                    continue;
                }
                match b[0] {
                    SHOW => on_show(),
                    QUIT => on_quit(),
                    _ => {}
                }
            }
        })
        .expect("spawn instance listener");
}
