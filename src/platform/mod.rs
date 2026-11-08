//! Platform layer: everything that talks to the OS. The core (app, view,
//! store, protocol, devices, drivers, mouse, keys, i18n) uses only the
//! API here; each backend implements it with the same plain structs and
//! functions, selected by `cfg`.

/// Start hidden in the tray (autostart launch line).
pub const TRAY_ARG: &str = "--tray";
pub const UNINSTALL_ARG: &str = "--uninstall";
/// Passed to the freshly installed copy so it waits for the old process to exit.
pub const INSTALLED_ARG: &str = "--installed";
pub const AUTOSTART_ON_ARG: &str = "--autostart-on";
pub const AUTOSTART_OFF_ARG: &str = "--autostart-off";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Autostart {
    On,
    Off,
}

pub enum Instance {
    First,
    AlreadyRunning,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// Hand over to a running instance.
    Now,
    /// Previous process is exiting (after install); wait for it.
    WaitForExit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Started from the OS app list (`--uninstall`); another instance may run.
    External,
    /// Clicked inside the running app, which quits right after.
    InApp,
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

