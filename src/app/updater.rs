//! Controller side of updates: when to check, what to show, install.
//!
//! ```text
//!   timer / About ──> thread: update::check() ──post──> on_checked
//!        offer: status bar notice, one notification per version
//!   Update ──> thread: download + verify + install ──post──> on_installed
//!        Ok: quit (the new exe waits for this one)   Err: dialog with release page
//! ```

use super::{Controller, DAY_SECS, SITE_URL, post};
use crate::i18n::{T, fill, tr};
use crate::platform::{self, UpdateKind, window};
use crate::stats;
use crate::store::{self, AppConfig};
use crate::update::{self, Check, Fail, Release};
use crate::view::{Dialog, Install, Offer, Probe, UpdateStep, note_lines, notes_cut};

/// How often the timer asks whether a daily check is due.
pub(super) const POLL_MS: u32 = 3_600_000;

/// Who started a check.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Trigger {
    /// Daily background check: silent unless something is new.
    Auto,
    /// About › Check for updates: every result is shown.
    Manual,
}

/// Update state the view does not hold.
#[derive(Default)]
pub(super) struct Updates {
    /// Newest release found, with its download.
    release: Option<Release>,
    checking: bool,
    downloading: bool,
}

/// What "Update" does here.
pub(super) fn install_kind() -> Install {
    match platform::UPDATE_KIND {
        UpdateKind::InPlace => Install::InPlace,
        UpdateKind::ReleasePage => Install::Page,
    }
}

/// A daily check is due: switched on and a day since the last one (or
/// the clock went back).
fn check_due(cfg: &AppConfig, now: u64) -> bool {
    cfg.update_check && (now < cfg.update_checked || now - cfg.update_checked >= DAY_SECS)
}

impl Controller {
    /// Timer and start: checks once a day while switched on.
    pub(super) fn auto_check(&mut self) {
        if check_due(&self.cfg, stats::unix_now()) {
            self.start_check(Trigger::Auto);
        }
    }

    pub(super) fn start_check(&mut self, t: Trigger) {
        if self.updates.checking || self.removed {
            return;
        }
        self.updates.checking = true;
        if t == Trigger::Manual {
            self.view.update.probe = Probe::Checking;
        }
        std::thread::spawn(move || {
            let r = update::check();
            post(move |c| c.on_checked(t, r));
        });
    }

    fn on_checked(&mut self, t: Trigger, r: Result<Check, Fail>) {
        self.updates.checking = false;
        // A failed check is retried at the next poll; an answer waits a day.
        if r.is_ok() && !self.removed {
            self.cfg.update_checked = stats::unix_now();
            store::save_config(&self.cfg);
        }
        let probe = match r {
            Ok(Check::Newer(rel)) => {
                self.offer_update(rel, t);
                Probe::Available
            }
            Ok(Check::Current) => Probe::Current,
            Ok(Check::NoInfo) | Err(_) => Probe::Failed,
        };
        // Background failures stay quiet.
        if t == Trigger::Manual || probe != Probe::Failed {
            self.view.update.probe = probe;
        }
        window::invalidate();
    }

    /// Status bar notice unless skipped; a notification once per version.
    fn offer_update(&mut self, rel: Release, t: Trigger) {
        let version = rel.version.to_string();
        let skipped = self.cfg.update_skip.as_deref() == Some(version.as_str());
        let offer = Offer { version: version.clone(), date: rel.date.clone(), notes: note_lines(&rel.notes), cut: notes_cut(&rel.notes) };
        if self.view.update.offer.as_ref() != Some(&offer) && !self.updates.downloading {
            self.view.update.step = UpdateStep::Ready;
        }
        self.view.update.offer = Some(offer);
        self.view.update.notice = !skipped;
        self.updates.release = Some(rel);

        let announced = self.cfg.update_notified.as_deref() == Some(version.as_str());
        if t == Trigger::Manual || skipped || announced || self.removed {
            return;
        }
        window::notify(tr(T::UpdateTitle), &fill(tr(T::UpdateAvailable), &[&version]));
        self.cfg.update_notified = Some(version);
        store::save_config(&self.cfg);
    }

    pub(super) fn show_update(&mut self) {
        if self.view.update.offer.is_some() {
            self.view.dialog = Some(Dialog::Update);
        }
    }

    /// Downloads and installs off the UI thread; macOS opens the page.
    pub(super) fn install_update(&mut self) {
        if install_kind() == Install::Page {
            platform::open_url(SITE_URL);
            return;
        }
        let Some(rel) = self.updates.release.clone().filter(|_| !self.updates.downloading && !self.removed) else {
            return;
        };
        self.updates.downloading = true;
        self.view.update.step = UpdateStep::Downloading;
        std::thread::spawn(move || {
            let r = update::download(&rel).and_then(|bytes| update::install(&rel, &bytes));
            post(move |c| c.on_installed(r));
        });
    }

    /// The new exe is in place and started: hand over. Else the error
    /// and the release page.
    fn on_installed(&mut self, r: Result<(), Fail>) {
        self.updates.downloading = false;
        if r.is_ok() {
            self.stats.save();
            window::quit();
            return;
        }
        self.view.update.step = UpdateStep::Failed;
        if self.view.dialog.is_none() {
            self.view.dialog = Some(Dialog::Update);
        }
        window::invalidate();
    }

    pub(super) fn skip_update(&mut self) {
        let Some(o) = &self.view.update.offer else {
            return;
        };
        self.cfg.update_skip = Some(o.version.clone());
        store::save_config(&self.cfg);
        self.view.update.notice = false;
        self.view.dialog = None;
    }

    pub(super) fn toggle_update_auto(&mut self) {
        self.cfg.update_check = !self.cfg.update_check;
        self.view.update.auto = self.cfg.update_check;
        store::save_config(&self.cfg);
        self.auto_check();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_once_a_day_while_on() {
        let mut cfg = AppConfig::default();
        let now = 1_800_000_000;
        assert!(check_due(&cfg, now));
        cfg.update_checked = now - DAY_SECS + 1;
        assert!(!check_due(&cfg, now));
        cfg.update_checked = now - DAY_SECS;
        assert!(check_due(&cfg, now));
        // Clock set back: check again rather than wait.
        cfg.update_checked = now + 10;
        assert!(check_due(&cfg, now));
        cfg.update_check = false;
        assert!(!check_due(&cfg, now - DAY_SECS * 2));
    }
}
