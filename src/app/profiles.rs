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
//! A profile belongs to the mouse it was made for; the pages and the tray
//! list only the chosen mouse's profiles, so `Act::Profile(i)` counts rows
//! of that list. Export writes the active profile or all of them to a
//! file the user picks; import adds a file's profiles to the list.

use super::{Controller, Undo};
use crate::i18n::{T, fill, tr};
use crate::mouse::Command;
use crate::platform::window;
use crate::protocol::DEFAULT_ACTIONS;
use crate::shell::Mods;
use crate::shell::FileAsk;
use crate::store::{self, AppConfig, PROFILE_MAX, PROFILE_NAME_MAX, PROFILE_TOTAL, Profile};
use std::path::PathBuf;
use crate::view::{Act, Dialog, Page, ProfileRow};

/// Extension of an exported file.
const PROFILE_EXT: &str = "whisker";

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

/// Model id of the chosen mouse; empty before the first choice.
fn device_of(cfg: &AppConfig) -> &str {
    cfg.device.as_deref().unwrap_or_default()
}

/// Indexes of the chosen mouse's profiles, in list order: the ones the
/// pages and the tray show.
fn own(cfg: &AppConfig) -> Vec<usize> {
    let device = device_of(cfg);
    cfg.profiles.iter().enumerate().filter(|(_, p)| p.device == device).map(|(i, _)| i).collect()
}

/// Rows of the Profiles page with the row of the active profile; the
/// active one counts the working layout.
fn rows(cfg: &AppConfig) -> (Vec<ProfileRow>, usize) {
    let own = own(cfg);
    let row = |&k: &usize| {
        let p = &cfg.profiles[k];
        let (buttons, macros) = if k == cfg.profile { (&cfg.buttons, &cfg.macros) } else { (&p.buttons, &p.macros) };
        ProfileRow {
            name: label(p),
            remapped: buttons.iter().zip(DEFAULT_ACTIONS).filter(|(a, d)| *a != d).count(),
            macros: macros.iter().filter(|m| !m.is_empty()).count(),
        }
    };
    (own.iter().map(row).collect(), own.iter().position(|&k| k == cfg.profile).unwrap_or(0))
}

/// Names of the profiles of `device`.
fn names_of<'a>(cfg: &'a AppConfig, device: &str) -> Vec<&'a str> {
    cfg.profiles.iter().filter(|p| p.device == device).map(|p| p.name.as_str()).collect()
}

/// "Profile N" with the lowest N none of `taken` has.
fn free_name(taken: &[&str]) -> String {
    let name = |n: usize| fill(tr(T::ProfileN), &[&n.to_string()]);
    name((1..).find(|&n| !taken.contains(&name(n).as_str())).unwrap_or(1))
}

/// `base` with the lowest free number from 2, e.g. "FPS" -> "FPS 2";
/// `base` is cut to leave room for the number.
fn numbered(taken: &[&str], base: &str) -> String {
    /// Room for " 2" .. " 9": a mouse has at most `PROFILE_MAX` profiles.
    const SUFFIX: usize = 2;
    let base: String = base.chars().take(PROFILE_NAME_MAX - SUFFIX).collect();
    let name = |n: usize| format!("{} {n}", base.trim_end());
    name((2..).find(|&n| !taken.contains(&name(n).as_str())).unwrap_or(2))
}

/// Appends a profile of the chosen mouse and makes it the active one;
/// `false` when the mouse's list, or the whole one, is full.
fn add(cfg: &mut AppConfig, seed: Seed) -> bool {
    if own(cfg).len() >= PROFILE_MAX || cfg.profiles.len() >= PROFILE_TOTAL {
        return false;
    }
    cfg.keep_active();
    let device = device_of(cfg).to_string();
    let taken = names_of(cfg, &device);
    let new = match seed {
        Seed::Blank => Profile { name: free_name(&taken), device: device.clone(), ..Profile::default() },
        Seed::Active => {
            let from = &cfg.profiles[cfg.profile];
            Profile { name: numbered(&taken, &label(from)), device: device.clone(), ..from.clone() }
        }
    };
    cfg.profiles.push(new);
    cfg.activate(cfg.profiles.len() - 1)
}

/// Deletes the active profile; the mouse's profile taking its place
/// becomes active. The mouse's last profile stays. Returns what Undo puts
/// back.
fn remove(cfg: &mut AppConfig) -> Option<Undo> {
    let before = own(cfg);
    let row = before.iter().position(|&k| k == cfg.profile)?;
    if before.len() <= 1 {
        return None;
    }
    cfg.keep_active();
    let at = cfg.profile;
    let gone = cfg.profiles.remove(at);
    let after = own(cfg);
    cfg.set_active(after[row.min(after.len() - 1)]);
    Some(Undo::Profile(at, gone))
}

/// Undo of `remove`: the profile is back at its place and active.
pub(super) fn restore(cfg: &mut AppConfig, at: usize, profile: Profile) {
    cfg.keep_active();
    let at = at.min(cfg.profiles.len());
    cfg.profiles.insert(at, profile);
    cfg.set_active(at);
}

/// Which profiles an export takes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// The active profile.
    Active,
    /// Every profile of every mouse: a backup.
    All,
}

/// The text of an export and the file name proposed for it.
fn export(cfg: &mut AppConfig, scope: Scope) -> (String, String) {
    const ALL_NAME: &str = "whisker-profiles";
    cfg.keep_active();
    let (picked, name): (Vec<&Profile>, String) = match scope {
        Scope::Active => (cfg.profiles.get(cfg.profile).into_iter().collect(), cfg.profiles.get(cfg.profile).map(label).unwrap_or_default()),
        Scope::All => (cfg.profiles.iter().collect(), ALL_NAME.into()),
    };
    (store::export_profiles(&picked), format!("{name}.{PROFILE_EXT}"))
}

/// What became of the profiles of an imported file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Imported {
    /// Added to the chosen mouse: they show in the list.
    here: usize,
    /// Added to other mice: shown once such a mouse is chosen.
    elsewhere: usize,
    /// The same profile is already there.
    same: usize,
    /// Made for a mouse Whisker does not know, or over a mouse's limit.
    skipped: usize,
    /// The working layout was replaced: it has to go to the mouse.
    layout: bool,
}

/// A profile as a mouse gets it at first: never named, nothing changed.
fn untouched(p: &Profile) -> bool {
    p.name.is_empty() && p.buttons == DEFAULT_ACTIONS && p.macros.iter().all(Vec::is_empty)
}

/// Marks profile `i` as its mouse's active one; on the chosen mouse it
/// becomes the working one.
fn make_current(cfg: &mut AppConfig, i: usize) {
    if cfg.profiles[i].device == device_of(cfg) {
        cfg.set_active(i);
        return;
    }
    let device = cfg.profiles[i].device.clone();
    for (k, p) in cfg.profiles.iter_mut().enumerate().filter(|(_, p)| p.device == device) {
        p.current = k == i;
    }
}

/// Adds the profiles of an imported file. A profile keeps the mouse it
/// was made for (one made for none goes to the chosen mouse) and waits
/// there until that mouse is chosen.
///
/// ```text
/// the same profile is already there     -> left out (a file imported twice)
/// its mouse has only the untouched one  -> takes its place; the file's
///                                          active mark holds (a restore)
/// its mouse is unknown, or has 8        -> skipped (`model` gives the
///                                          present id of a known model
///                                          with button settings)
/// the name is taken on its mouse        -> gets a number, "FPS 2"
/// ```
fn merge(cfg: &mut AppConfig, incoming: Vec<Profile>, model: impl Fn(&str) -> Option<String>) -> Imported {
    cfg.keep_active();
    let chosen = device_of(cfg).to_string();
    let mut out = Imported::default();
    // Mice whose untouched profile gave way: the file says which is active.
    let mut restored: Vec<String> = Vec::new();

    for mut p in incoming {
        if p.device.is_empty() {
            p.device = chosen.clone();
        }
        let same = |q: &Profile| (&q.device, &q.name, &q.buttons, &q.macros) == (&p.device, &p.name, &p.buttons, &p.macros);
        if cfg.profiles.iter().any(same) {
            out.same += 1;
            continue;
        }
        // Only a mouse with button settings has profiles; an old id of
        // the model becomes its present one.
        let Some(id) = model(&p.device) else {
            out.skipped += 1;
            continue;
        };
        p.device = id;

        let theirs: Vec<usize> = cfg.profiles.iter().enumerate().filter(|(_, q)| q.device == p.device).map(|(i, _)| i).collect();
        // A mouse with nothing of its own yet is restored from the file.
        let fresh = theirs.len() == 1 && untouched(&cfg.profiles[theirs[0]]) && !restored.contains(&p.device);
        if theirs.is_empty() || fresh {
            restored.push(p.device.clone());
            p.current = true;
        }
        let at = if fresh {
            theirs[0]
        } else if theirs.len() >= PROFILE_MAX || cfg.profiles.len() >= PROFILE_TOTAL {
            out.skipped += 1;
            continue;
        } else {
            cfg.profiles.len()
        };

        let names: Vec<&str> = theirs.iter().filter(|_| !fresh).map(|&i| cfg.profiles[i].name.as_str()).collect();
        if names.contains(&p.name.as_str()) {
            p.name = numbered(&names, &label(&p));
        }
        let active = std::mem::take(&mut p.current) && restored.contains(&p.device);
        let here = p.device == chosen;
        match fresh {
            true => cfg.profiles[at] = p,
            false => cfg.profiles.push(p),
        }
        if active {
            make_current(cfg, at);
            out.layout |= here;
        }
        match here {
            true => out.here += 1,
            false => out.elsewhere += 1,
        }
    }
    out
}

/// The present id of a catalogue model that has button settings.
fn with_buttons(id: &str) -> Option<String> {
    crate::devices::by_id(id).filter(|m| m.caps.buttons).map(|m| m.id.clone())
}

/// The toast after an import: what was added here, and what the list
/// does not show.
fn import_note(i: Imported) -> String {
    if i == Imported::default() {
        return tr(T::ToastImportNone).into();
    }
    let mut note = fill(tr(T::ToastImported), &[&i.here.to_string()]);
    for (count, part) in [(i.elsewhere, T::ImportElsewhere), (i.same, T::ImportSame), (i.skipped, T::ImportSkipped)] {
        if count > 0 {
            note += &format!(" · {}", fill(tr(part), &[&count.to_string()]));
        }
    }
    note
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
        (self.view.profiles, self.view.profile) = rows(&self.cfg);
        let names: Vec<String> = if self.view.caps.buttons { self.view.profiles.iter().map(|r| r.name.clone()).collect() } else { Vec::new() };
        window::set_tray_profiles(&names, self.view.profile);
    }

    pub(super) fn profile_act(&mut self, a: Act) {
        match a {
            Act::Profile(i) => self.switch_profile(i),
            Act::ProfileNew => self.add_profile(Seed::Blank),
            Act::ProfileDuplicate => self.add_profile(Seed::Active),
            Act::ProfileRename => self.view.rename = Some(String::new()),
            Act::ProfileDelete => self.delete_profile(),
            Act::ProfileExport => self.export_profiles(Scope::Active),
            Act::ProfileExportAll => self.export_profiles(Scope::All),
            Act::ProfileImport => self.import_profiles(),
            _ => {}
        }
    }

    /// Makes row `row` of the chosen mouse's profiles the active one and
    /// sends its layout to the mouse.
    pub(super) fn switch_profile(&mut self, row: usize) {
        let Some(&i) = own(&self.cfg).get(row) else {
            return;
        };
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
        let name = self.view.profiles.get(self.view.profile).map(|r| r.name.clone()).unwrap_or_default();
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
        let old = self.view.profiles.get(self.view.profile).map(|r| r.name.clone()).unwrap_or_default();
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

    /// Asks where to save, then writes the profiles there. The dialog opens
    /// after this event; its answer comes back through the controller.
    fn export_profiles(&mut self, scope: Scope) {
        if std::mem::replace(&mut self.asking_file, true) {
            return;
        }
        let (text, name) = export(&mut self.cfg, scope);
        let title = tr(if scope == Scope::All { T::ProfileExportAll } else { T::ProfileExport });
        window::ask_file(FileAsk::Save, title, &name, PROFILE_EXT, move |path| super::post(move |c| c.on_export(path, &text)));
    }

    fn on_export(&mut self, path: Option<PathBuf>, text: &str) {
        self.asking_file = false;
        let Some(path) = path else {
            return;
        };
        let note = match store::write_export(&path, text) {
            true => fill(tr(T::ToastExported), &[&path.file_name().unwrap_or_default().to_string_lossy()]),
            false => tr(T::ToastExportFailed).into(),
        };
        self.show_toast(note, None);
    }

    fn import_profiles(&mut self) {
        if std::mem::replace(&mut self.asking_file, true) {
            return;
        }
        window::ask_file(FileAsk::Open, tr(T::ProfileImport), "", PROFILE_EXT, |path| super::post(move |c| c.on_import(path)));
    }

    /// Adds the profiles of the picked file and says where they went.
    fn on_import(&mut self, path: Option<PathBuf>) {
        self.asking_file = false;
        let Some(path) = path else {
            return;
        };
        let incoming = store::read_import(&path).map(|text| store::import_profiles(&text)).unwrap_or_default();
        self.leave_layout();
        let got = merge(&mut self.cfg, incoming, with_buttons);
        crate::log!("profiles imported: {got:?}");
        if got.layout {
            // A restore put another layout to work: the mouse gets it.
            self.layout_changed();
        } else {
            store::save_config(&self.cfg);
            self.sync_profiles();
        }
        self.show_toast(import_note(got), None);
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
        let (r, _) = rows(&cfg);
        assert_eq!((r[0].remapped, r[0].macros), (1, 1));
        assert_eq!((r[1].remapped, r[1].macros), (0, 0));
        assert_eq!(r[0].name, "Default");
    }

    const LIONEL: &str = "whiteshark/lionel";
    const OTHER: &str = "eternico/aet-ms480b";
    /// The two mice of the tests are known; any other is not.
    fn known(id: &str) -> Option<String> {
        [LIONEL, OTHER].contains(&id).then(|| id.to_string())
    }

    /// The default layout with Back and Forward swapped.
    const SWAPPED: [Action; crate::protocol::BUTTON_COUNT] =
        [DEFAULT_ACTIONS[0], DEFAULT_ACTIONS[1], DEFAULT_ACTIONS[2], DEFAULT_ACTIONS[4], DEFAULT_ACTIONS[3], DEFAULT_ACTIONS[5]];

    /// A config whose chosen mouse is `device`, with its first profile.
    fn on_mouse(device: &str) -> AppConfig {
        let mut cfg = AppConfig { device: Some(device.into()), ..AppConfig::default() };
        cfg.use_device(device);
        cfg
    }

    #[test]
    fn the_list_is_the_chosen_mouses_own() {
        let mut cfg = on_mouse(LIONEL);
        assert!(add(&mut cfg, Seed::Blank));
        cfg.device = Some(OTHER.into());
        cfg.use_device(OTHER);
        assert!(add(&mut cfg, Seed::Blank));

        // Two of the Lionel, two of the other mouse: each sees its own.
        assert_eq!((cfg.profiles.len(), own(&cfg)), (4, vec![2, 3]));
        let (listed, active) = rows(&cfg);
        assert_eq!((listed.len(), active), (2, 1));
        // The new one is numbered among its mouse's names only.
        assert_eq!(cfg.profiles[3].name, "Profile 1");

        // Deleting leaves the other mouse's profiles alone.
        assert!(remove(&mut cfg).is_some());
        assert_eq!((cfg.profile, cfg.profiles[cfg.profile].device.as_str()), (2, OTHER));
        assert_eq!(remove(&mut cfg), None, "the mouse's last profile stays");
        assert_eq!(cfg.profiles.len(), 3);
    }

    #[test]
    fn export_takes_the_active_profile_or_all() {
        let mut cfg = on_mouse(LIONEL);
        assert!(add(&mut cfg, Seed::Blank));
        cfg.buttons[4] = Action::Disable;

        let (text, name) = export(&mut cfg, Scope::Active);
        assert_eq!(name, "Profile 1.whisker");
        let one = store::import_profiles(&text);
        assert_eq!(one.len(), 1);
        assert_eq!((one[0].device.as_str(), one[0].buttons[4]), (LIONEL, Action::Disable), "with the edit not yet kept");

        let (text, name) = export(&mut cfg, Scope::All);
        assert_eq!((store::import_profiles(&text).len(), name.as_str()), (2, "whisker-profiles.whisker"));
    }

    #[test]
    fn import_adds_profiles_to_their_mouse() {
        let mut cfg = on_mouse(LIONEL);
        cfg.profiles[0].name = "Work".into();
        let file = vec![
            // An own name; made for no mouse; for another mouse; for an unknown one.
            Profile { name: "Work".into(), device: LIONEL.into(), current: true, buttons: SWAPPED, ..Profile::default() },
            Profile { name: "Free".into(), ..Profile::default() },
            Profile { name: "Desk".into(), device: OTHER.into(), ..Profile::default() },
            Profile { name: "Ghost".into(), device: "nobody/none".into(), ..Profile::default() },
        ];
        let got = merge(&mut cfg, file.clone(), known);
        assert_eq!(got, Imported { here: 2, elsewhere: 1, skipped: 1, ..Imported::default() });
        let listed: Vec<(&str, &str)> = cfg.profiles.iter().map(|p| (p.name.as_str(), p.device.as_str())).collect();
        assert_eq!(listed, [("Work", LIONEL), ("Work 2", LIONEL), ("Free", LIONEL), ("Desk", OTHER)]);
        // A mouse in use keeps its active profile; one new here gets its first.
        assert_eq!(cfg.profile, 0);
        assert_eq!(cfg.profiles.iter().map(|p| p.current).collect::<Vec<_>>(), [true, false, false, true]);

        // "Free" and "Desk" are there now; "Work" differs from both of that name.
        let again = merge(&mut cfg, file, known);
        assert_eq!(again, Imported { here: 1, same: 2, skipped: 1, ..Imported::default() });
    }

    #[test]
    fn a_backup_is_restored_over_untouched_profiles() {
        // The backup: two profiles of the Lionel, the second active; one of another mouse.
        let mut old = on_mouse(LIONEL);
        old.buttons = SWAPPED;
        assert!(add(&mut old, Seed::Blank));
        old.profiles[1].name = "FPS".into();
        old.profiles.push(Profile { name: "Desk".into(), device: OTHER.into(), current: true, ..Profile::default() });
        let (text, _) = export(&mut old, Scope::All);

        // A new installation: the mouse has its untouched starting profile.
        let mut cfg = on_mouse(LIONEL);
        let got = merge(&mut cfg, store::import_profiles(&text), known);
        assert_eq!(got, Imported { here: 2, elsewhere: 1, layout: true, ..Imported::default() });
        assert_eq!(cfg.profiles, old.profiles, "as it was, no leftover default");
        assert_eq!((cfg.profile, cfg.buttons), (1, DEFAULT_ACTIONS));

        // The same file again adds nothing.
        let again = merge(&mut cfg, store::import_profiles(&text), known);
        assert_eq!(again, Imported { same: 3, ..Imported::default() });
        assert_eq!(cfg.profiles.len(), 3);
    }

    #[test]
    fn import_stops_at_a_mouses_limit() {
        let mut cfg = on_mouse(LIONEL);
        cfg.profiles[0].name = "Mine".into();
        let many: Vec<Profile> = (0..PROFILE_MAX + 3).map(|n| Profile { name: format!("P{n}"), device: LIONEL.into(), ..Profile::default() }).collect();
        let got = merge(&mut cfg, many, known);
        assert_eq!((got.here, got.skipped), (PROFILE_MAX - 1, 4));
        assert_eq!(own(&cfg).len(), PROFILE_MAX);
    }

    #[test]
    fn the_import_note_says_what_the_list_does_not_show() {
        assert_eq!(import_note(Imported::default()), "No profiles to import in this file.");
        assert_eq!(import_note(Imported { here: 2, ..Imported::default() }), "Profiles imported: 2");
        let mixed = Imported { here: 0, elsewhere: 5, same: 1, skipped: 2, layout: false };
        assert_eq!(import_note(mixed), "Profiles imported: 0 · for other mice: 5 · already there: 1 · skipped: 2");
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
