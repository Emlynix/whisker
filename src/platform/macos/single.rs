//! One running instance. An exclusive lock on a file in the per-user temp
//! folder marks the running instance (the OS drops it when the process
//! ends, also on a crash). A second launch tells the first to show its
//! window, the uninstaller tells it to quit, both via distributed
//! notifications of the user's session.

use super::sys::temp_dir;
use crate::platform::{Claim, Instance};
use block2::RcBlock;
use objc2_foundation::{NSDistributedNotificationCenter, NSNotification, NSOperationQueue, NSString};
use std::fs::File;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const LOCK_NAME: &str = "com.emlynix.whisker.lock";
const SHOW_EVENT: &str = "com.emlynix.whisker.show";
const QUIT_EVENT: &str = "com.emlynix.whisker.quit";
const RETRY_GAP: Duration = Duration::from_millis(100);
const WAIT_LIMIT: Duration = Duration::from_secs(5);

/// Held for the life of the process.
static LOCK: OnceLock<File> = OnceLock::new();

fn lock_path() -> Option<PathBuf> {
    Some(temp_dir()?.join(LOCK_NAME))
}

/// Opens the lock file and takes the exclusive lock if free.
fn try_lock() -> Option<File> {
    let f = File::options().create(true).truncate(false).write(true).open(lock_path()?).ok()?;
    f.try_lock().ok()?;
    Some(f)
}

fn signal(event: &str) {
    let name = NSString::from_str(event);
    let center = NSDistributedNotificationCenter::defaultCenter();
    // SAFETY: no user info; deliver even while the receiver is inactive.
    unsafe { center.postNotificationName_object_userInfo_deliverImmediately(&name, None, None, true) };
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

    signal(SHOW_EVENT);
    Instance::AlreadyRunning
}

/// Asks a running instance to exit (used by the uninstaller).
pub fn request_quit() {
    signal(QUIT_EVENT);
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

fn on_event(name: &str, f: impl Fn() + Send + 'static) {
    let block = RcBlock::new(move |_n: NonNull<NSNotification>| f());
    let center = NSDistributedNotificationCenter::defaultCenter();
    let name = NSString::from_str(name);
    // SAFETY: delivered on the main queue; `f` is `Send`.
    let token = unsafe { center.addObserverForName_object_queue_usingBlock(Some(&name), None, Some(&NSOperationQueue::mainQueue()), &block) };
    // The observer stays for the whole process.
    std::mem::forget(token);
}

/// Calls `on_show` / `on_quit` when another process asks for it.
pub fn listen(on_show: impl Fn() + Send + 'static, on_quit: impl Fn() + Send + 'static) {
    on_event(SHOW_EVENT, on_show);
    on_event(QUIT_EVENT, on_quit);
}
