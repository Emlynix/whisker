//! Profiles: named button layouts with their macros. One is active; its
//! layout is what the Buttons and Macros pages edit and what the mouse
//! has. Switching sends the other layout to the mouse.
//!
//! ```text
//! Profiles page · Overview · tray menu ── Act::Profile(i) / TrayAction::Profile(i)
//!        │
//!        ▼
//! switch_profile ── AppConfig::activate ── SetButtons to the mouse ── config.txt
//! ```

use super::{Controller, Undo};
use crate::i18n::{T, fill, tr};
use crate::mouse::Command;
use crate::platform::window;
use crate::protocol::DEFAULT_ACTIONS;
use crate::shell::Mods;
use crate::store::{self, AppConfig, PROFILE_MAX, PROFILE_NAME_MAX, Profile};
use crate::view::{Act, Dialog, Page, ProfileRow};

const VK_BACK: u16 = 0x08;
const VK_RETURN: u16 = 0x0D;
const VK_ESCAPE: u16 = 0x1B;
const VK_SPACE: u16 = 0x20;
const VK_0: u16 = 0x30;
const VK_9: u16 = 0x39;
const VK_A: u16 = 0x41;
const VK_Z: u16 = 0x5A;
const VK_NUMPAD0: u16 = 0x60;
const VK_NUMPAD9: u16 = 0x69;
const VK_MINUS: u16 = 0xBD;
const VK_PERIOD: u16 = 0xBE;

/// What a new profile starts from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Seed {
    /// The default layout, no macros.
    Blank,
    /// A copy of the active profile.
    Active,
}

/// The name shown: a profile never named is "Default".
fn label(p: &Profile) -> String {
    if p.name.is_empty() { tr(T::ProfileDefault).into() } else { p.name.clone() }
}

/// Rows of the Profiles page; the active one counts the working layout.
fn rows(cfg: &AppConfig) -> Vec<ProfileRow> {
    let row = |(k, p): (usize, &Profile)| {
        let (buttons, macros) = if k == cfg.profile { (&cfg.buttons, &cfg.macros) } else { (&p.buttons, &p.macros) };
        ProfileRow {
            name: label(p),
            remapped: buttons.iter().zip(DEFAULT_ACTIONS).filter(|(a, d)| *a != d).count(),
            macros: macros.iter().filter(|m| !m.is_empty()).count(),
        }
    };
    cfg.profiles.iter().enumerate().map(row).collect()
}

/// "Profile N" with the lowest N no profile has yet.
fn free_name(profiles: &[Profile]) -> String {
    let name = |n: usize| fill(tr(T::ProfileN), &[&n.to_string()]);
    let taken = |n: &usize| profiles.iter().any(|p| p.name == name(*n));
    name((1..).find(|n| !taken(n)).unwrap_or(1))
}

/// Name of a copy: the original's with the lowest free number from 2,
/// e.g. "FPS" -> "FPS 2"; the original's name is cut to leave room for it.
fn copy_name(profiles: &[Profile], of: &Profile) -> String {
    /// Room for " 2" .. " 9": there are at most `PROFILE_MAX` profiles.
    const SUFFIX: usize = 2;
    let base: String = label(of).chars().take(PROFILE_NAME_MAX - SUFFIX).collect();
    let name = |n: usize| format!("{} {n}", base.trim_end());
    let taken = |n: &usize| profiles.iter().any(|p| p.name == name(*n));
    name((2..).find(|n| !taken(n)).unwrap_or(2))
}

/// Appends a profile and makes it the active one; `false` when the list
/// is full.
fn add(cfg: &mut AppConfig, seed: Seed) -> bool {
    if cfg.profiles.len() >= PROFILE_MAX {
        return false;
    }
    cfg.keep_active();
    let new = match seed {
        Seed::Blank => Profile { name: free_name(&cfg.profiles), ..Profile::default() },
        Seed::Active => {
            let from = &cfg.profiles[cfg.profile];
            Profile { name: copy_name(&cfg.profiles, from), ..from.clone() }
        }
    };
    cfg.profiles.push(new);
    cfg.activate(cfg.profiles.len() - 1)
}

/// Makes `i` the active profile without keeping the working layout.
fn load(cfg: &mut AppConfig, i: usize) {
    cfg.profile = i;
    (cfg.buttons, cfg.macros) = (cfg.profiles[i].buttons, cfg.profiles[i].macros.clone());
}

/// Deletes the active profile; the one taking its place becomes active.
/// The last profile stays. Returns what Undo puts back.
fn remove(cfg: &mut AppConfig) -> Option<Undo> {
    if cfg.profiles.len() <= 1 {
        return None;
    }
    cfg.keep_active();
    let at = cfg.profile;
    let gone = cfg.profiles.remove(at);
    load(cfg, at.min(cfg.profiles.len() - 1));
    Some(Undo::Profile(at, gone))
}

/// Undo of `remove`: the profile is back at its place and active.
pub(super) fn restore(cfg: &mut AppConfig, at: usize, profile: Profile) {
    cfg.keep_active();
    let at = at.min(cfg.profiles.len());
    cfg.profiles.insert(at, profile);
    load(cfg, at);
}

/// The character a key types into a profile name: letters, digits, space,
/// `-` and `.` (what `store::clean_name` keeps). The app gets keys, not
/// text, so other characters cannot be typed. Digits come with or without
/// Shift: Slovak and Czech keyboards have them on Shift.
fn typed(vk: u16, m: Mods) -> Option<char> {
    if m.ctrl || m.alt || m.win {
        return None;
    }
    let ascii = |base: u8, from: u16| (base + (vk - from) as u8) as char;
    match vk {
        VK_A..=VK_Z if m.shift => Some(ascii(b'A', VK_A)),
        VK_A..=VK_Z => Some(ascii(b'a', VK_A)),
        VK_0..=VK_9 => Some(ascii(b'0', VK_0)),
        VK_NUMPAD0..=VK_NUMPAD9 => Some(ascii(b'0', VK_NUMPAD0)),
        VK_SPACE => Some(' '),
        VK_MINUS if !m.shift => Some('-'),
        VK_PERIOD if !m.shift => Some('.'),
        _ => None,
    }
}

impl Controller {
    /// Profile names and counts for the pages, and the tray's list. A mouse
    /// without button settings has no use for profiles: no list.
    pub(super) fn sync_profiles(&mut self) {
        self.view.profiles = rows(&self.cfg);
        self.view.profile = self.cfg.profile;
        let names: Vec<String> = if self.view.caps.buttons { self.view.profiles.iter().map(|r| r.name.clone()).collect() } else { Vec::new() };
        window::set_tray_profiles(&names, self.cfg.profile);
    }

    pub(super) fn profile_act(&mut self, a: Act) {
        match a {
            Act::Profile(i) => self.switch_profile(i),
            Act::ProfileNew => self.add_profile(Seed::Blank),
            Act::ProfileDuplicate => self.add_profile(Seed::Active),
            Act::ProfileRename => self.view.rename = Some(String::new()),
            Act::ProfileDelete => self.delete_profile(),
            _ => {}
        }
    }

    /// Makes profile `i` the active one and sends its layout to the mouse.
    pub(super) fn switch_profile(&mut self, i: usize) {
        if !self.view.caps.buttons || i == self.cfg.profile {
            return;
        }
        self.leave_layout();
        if self.cfg.activate(i) {
            self.layout_changed();
        }
    }

    fn add_profile(&mut self, seed: Seed) {
        self.leave_layout();
        if add(&mut self.cfg, seed) {
            self.layout_changed();
        }
    }

    fn delete_profile(&mut self) {
        self.leave_layout();
        let name = self.view.profiles.get(self.cfg.profile).map(|r| r.name.clone()).unwrap_or_default();
        let Some(undo) = remove(&mut self.cfg) else {
            return;
        };
        self.layout_changed();
        self.show_toast(fill(tr(T::ToastProfileDeleted), &[&name]), Some(undo));
    }

    /// Ends what edits the working layout: a recording, an open picker, and
    /// an undo that belongs to this layout.
    fn leave_layout(&mut self) {
        self.stop_recording();
        self.view.picker = None;
        if self.view.dialog == Some(Dialog::AssignMacro) {
            self.view.dialog = None;
        }
        self.drop_undo();
    }

    /// Another layout is the working one: show it, send it, save it.
    pub(super) fn layout_changed(&mut self) {
        self.view.buttons = self.cfg.buttons;
        self.view.macros = self.cfg.macros.clone();
        self.view.notice.clear();
        self.send_setting(Command::SetButtons(self.cfg.buttons, self.cfg.macros.clone()));
        store::save_config(&self.cfg);
        self.sync_profiles();
        window::invalidate();
    }

    /// A key while a profile name is typed; `false` when none is. The
    /// field lives on the Profiles page only: anywhere else it is over.
    pub(super) fn rename_key(&mut self, vk: u16, m: Mods) -> bool {
        if self.view.page != Page::Profiles || self.modal() {
            self.end_rename();
            return false;
        }
        let old = self.view.profiles.get(self.cfg.profile).map(|r| r.name.clone()).unwrap_or_default();
        let Some(text) = self.view.rename.as_mut() else {
            return false;
        };
        match vk {
            VK_RETURN => {
                self.end_rename();
            }
            VK_ESCAPE => self.view.rename = None,
            // Nothing typed yet: Backspace edits the old name.
            VK_BACK if text.is_empty() => {
                *text = old;
                text.pop();
            }
            VK_BACK => {
                text.pop();
            }
            _ => {
                if let Some(c) = typed(vk, m).filter(|_| text.chars().count() < PROFILE_NAME_MAX) {
                    text.push(c);
                }
            }
        }
        true
    }

    /// Stores the typed name; nothing typed keeps the old one. `true` if a
    /// name was being typed.
    pub(super) fn end_rename(&mut self) -> bool {
        let Some(text) = self.view.rename.take() else {
            return false;
        };
        let name = store::clean_name(&text);
        if name.is_empty() {
            return true;
        }
        if let Some(p) = self.cfg.profiles.get_mut(self.cfg.profile) {
            p.name = name;
        }
        store::save_config(&self.cfg);
        self.sync_profiles();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Action;

    #[test]
    fn a_new_profile_is_blank_and_active() {
        let mut cfg = AppConfig::default();
        cfg.buttons[4] = Action::Disable;
        assert!(add(&mut cfg, Seed::Blank));
        assert_eq!((cfg.profiles.len(), cfg.profile), (2, 1));
        assert_eq!(cfg.buttons, DEFAULT_ACTIONS);
        // The layout that was being edited stays with its profile.
        assert_eq!(cfg.profiles[0].buttons[4], Action::Disable);
        assert_eq!(cfg.profiles[1].name, "Profile 1");
    }

    #[test]
    fn a_duplicate_copies_the_working_layout() {
        let mut cfg = AppConfig::default();
        cfg.buttons[4] = Action::Disable;
        assert!(add(&mut cfg, Seed::Active));
        assert_eq!(cfg.buttons[4], Action::Disable);
        assert_eq!(cfg.profiles[1].name, "Default 2");
        assert!(cfg.activate(0));
        assert!(add(&mut cfg, Seed::Active));
        assert_eq!(cfg.profiles[2].name, "Default 3");
        assert!(add(&mut cfg, Seed::Blank));
        assert_eq!(cfg.profiles[3].name, "Profile 1");

        // A long name is cut so the number fits.
        cfg.profiles[3].name = "A".repeat(PROFILE_NAME_MAX);
        assert!(add(&mut cfg, Seed::Active));
        assert_eq!(cfg.profiles[4].name, format!("{} 2", "A".repeat(PROFILE_NAME_MAX - 2)));
    }

    #[test]
    fn the_list_is_full_at_its_limit() {
        let mut cfg = AppConfig::default();
        while add(&mut cfg, Seed::Blank) {}
        assert_eq!(cfg.profiles.len(), PROFILE_MAX);
    }

    #[test]
    fn delete_activates_the_neighbour_and_undo_puts_it_back() {
        let mut cfg = AppConfig::default();
        assert!(add(&mut cfg, Seed::Blank));
        cfg.buttons[3] = Action::Disable;
        let Some(Undo::Profile(at, gone)) = remove(&mut cfg) else {
            panic!("a second profile can be deleted");
        };
        assert_eq!((cfg.profiles.len(), cfg.profile, at), (1, 0, 1));
        assert_eq!(cfg.buttons, DEFAULT_ACTIONS);
        assert_eq!(gone.buttons[3], Action::Disable, "deleted with its last edit");

        restore(&mut cfg, at, gone);
        assert_eq!((cfg.profiles.len(), cfg.profile), (2, 1));
        assert_eq!(cfg.buttons[3], Action::Disable);
    }

    #[test]
    fn the_last_profile_stays() {
        let mut cfg = AppConfig::default();
        assert_eq!(remove(&mut cfg), None);
        assert_eq!(cfg.profiles.len(), 1);
    }

    #[test]
    fn rows_count_what_a_profile_changes() {
        use crate::protocol::{Edge, Ev, MacroEvent};
        let mut cfg = AppConfig::default();
        cfg.buttons[4] = Action::Disable;
        cfg.macros[0] = vec![MacroEvent { ev: Ev::Key(0x04, Edge::Down), delay_ms: 10 }];
        assert!(add(&mut cfg, Seed::Blank));
        let r = rows(&cfg);
        assert_eq!((r[0].remapped, r[0].macros), (1, 1));
        assert_eq!((r[1].remapped, r[1].macros), (0, 0));
        assert_eq!(r[0].name, "Default");
    }

    #[test]
    fn a_name_is_typed_from_keys() {
        let (plain, shift) = (Mods::default(), Mods { shift: true, ..Mods::default() });
        let word: String = [(0x46, shift), (0x50, plain), (0x53, plain), (VK_SPACE, plain), (0x32, plain), (0x33, shift), (VK_MINUS, plain), (VK_MINUS, shift)]
            .into_iter()
            .filter_map(|(vk, m)| typed(vk, m))
            .collect();
        assert_eq!(word, "Fps 23-");
        // Shortcuts and other keys type nothing.
        assert_eq!(typed(VK_A, Mods { ctrl: true, ..Mods::default() }), None);
        assert_eq!(typed(0x70, plain), None);
    }
}
