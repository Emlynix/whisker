//! Buttons page, macro recorder page and the "assign action" dialog.
//!
//! ```text
//!  Buttons page                      Assign dialog (modal)
//!  +--------+ +------------------+   +--------+--------------------------+
//!  |  .-.   | | 1 Left button  > |   | Mouse  |  [Left click] [Right]    |
//!  | 1|3|2  | |   Left click     |   | Media  |  [Middle]     [Back] …   |
//!  |  |6|   | | 2 Right button • |   | …      |                 [Cancel] |
//!  | 5    4 | | …                |   +--------+--------------------------+
//!  +--------+ +------------------+
//!  Hovering or focusing a row lights its marker, and the other way round.
//! ```

use super::{Act, BODY, CW, Frame, Glyph, HEIGHT, Mark, PAGE_H, PANEL_GAP, PANEL_PAD, R_CTRL, RADIUS, SEGMENT_H, Segment, Squares, View, WIDTH, X0, art, glyph, mesh, name_lines, text_w, th};
use crate::gfx::{Align, Color, Rect, Size, Weight};
use crate::i18n::{T, fill, tr};
use crate::keys;
use crate::protocol::{
    Action, DEFAULT_ACTIONS, DpiAction, Edge, Ev, MACRO_COUNT, MB_BACK, MB_FORWARD, MB_LEFT, MB_MIDDLE, MB_RIGHT,
    MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN, MacroEvent, MacroMode,
};

const ROW: f32 = 50.0;
const PAD: f32 = 20.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Mouse,
    Media,
    Shortcuts,
    Keyboard,
    Fire,
    Macro,
    Other,
}

const TABS: [(Tab, T); 7] = [
    (Tab::Mouse, T::TabMouse),
    (Tab::Media, T::TabMedia),
    (Tab::Shortcuts, T::TabShortcuts),
    (Tab::Keyboard, T::TabKeyboard),
    (Tab::Fire, T::TabFire),
    (Tab::Macro, T::TabMacro),
    (Tab::Other, T::TabOther),
];

/// Delay written between recorded macro steps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DelayMode {
    Recorded,
    Fixed(u16),
}

pub const DELAY_CHOICES: [DelayMode; 4] = [DelayMode::Recorded, DelayMode::Fixed(10), DelayMode::Fixed(50), DelayMode::Fixed(100)];
pub const FIRE_INTERVALS: [u8; 5] = [10, 20, 30, 50, 100];
/// 0 = repeat while held.
pub const FIRE_LOOPS: [u8; 5] = [0, 2, 3, 5, 10];
pub const MACRO_MODES: [MacroMode; 5] = [
    MacroMode::Times(1), MacroMode::Times(3), MacroMode::Times(5), MacroMode::WhileHeld, MacroMode::Toggle,
];

/// State of the open "assign action" dialog.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Picker {
    pub button: usize,
    pub tab: Tab,
    pub mods: u8,
    pub key: Option<u8>,
    /// Waiting for the next key press.
    pub capturing: bool,
    /// `MOD_*` keys pressed during the capture.
    pub held: u8,
    pub interval: u8,
    pub loops: u8,
    pub macro_index: usize,
    pub mode: MacroMode,
    /// Preset picked on a one-click tab, applied with "Apply".
    pub chosen: Option<Action>,
}

impl Picker {
    /// Opens on the tab matching the button's current action, prefilled.
    pub fn for_button(button: usize, current: Action) -> Picker {
        let mut p = Picker {
            button,
            tab: Tab::Mouse,
            mods: MOD_CTRL,
            key: None,
            capturing: false,
            held: 0,
            interval: 50,
            loops: 0,
            macro_index: 0,
            mode: MacroMode::Times(1),
            chosen: None,
        };
        match current {
            Action::Consumer(_) => p.tab = Tab::Media,
            Action::Key { mods, usage } => {
                p.tab = if SHORTCUTS.iter().any(|s| (s.1, s.2) == (mods, usage)) { Tab::Shortcuts } else { Tab::Keyboard };
                (p.mods, p.key) = (mods, Some(usage));
            }
            Action::Fire { interval, loops } => (p.tab, p.interval, p.loops) = (Tab::Fire, interval, loops),
            Action::Macro { index, mode } => (p.tab, p.macro_index, p.mode) = (Tab::Macro, index, mode),
            Action::Disable | Action::SwitchEffect => p.tab = Tab::Other,
            Action::Mouse(_) | Action::Dpi(_) => {}
        }
        p
    }

    /// Action the "Apply" button assigns; `None` until one is complete.
    /// Every tab applies the same way: pick, then Apply.
    pub fn built(&self) -> Option<Action> {
        match self.tab {
            Tab::Keyboard => self.key.map(|usage| Action::Key { mods: self.mods, usage }),
            Tab::Fire => Some(Action::Fire { interval: self.interval, loops: self.loops }),
            Tab::Macro => Some(Action::Macro { index: self.macro_index, mode: self.mode }),
            Tab::Mouse | Tab::Media | Tab::Shortcuts | Tab::Other => self.chosen,
        }
    }
}

const BUTTON_NAMES: [T; 6] = [T::BtnLeft, T::BtnRight, T::BtnMiddle, T::BtnBack, T::BtnForward, T::BtnDpi];

pub fn button_name(i: usize) -> &'static str {
    tr(BUTTON_NAMES[i.min(BUTTON_NAMES.len() - 1)])
}

const MOUSE_ACTIONS: [(T, Action); 8] = [
    (T::ActLeft, Action::Mouse(MB_LEFT)),
    (T::ActRight, Action::Mouse(MB_RIGHT)),
    (T::ActMiddle, Action::Mouse(MB_MIDDLE)),
    (T::ActBack, Action::Mouse(MB_BACK)),
    (T::ActForward, Action::Mouse(MB_FORWARD)),
    (T::ActDpiLoop, Action::Dpi(DpiAction::Loop)),
    (T::ActDpiLoop, Action::Dpi(DpiAction::Up)),
    (T::ActDpiLoop, Action::Dpi(DpiAction::Down)),
];

/// HID consumer usages of the vendor's multimedia presets.
const MEDIA: [(T, u16); 11] = [
    (T::PlayPause, 0x00CD),
    (T::NextTrack, 0x00B5),
    (T::PrevTrack, 0x00B6),
    (T::StopMedia, 0x00B7),
    (T::VolumeUp, 0x00E9),
    (T::VolumeDown, 0x00EA),
    (T::Mute, 0x00E2),
    (T::MediaPlayer, 0x0183),
    (T::Mail, 0x018A),
    (T::Calculator, 0x0192),
    (T::ThisPc, 0x0194),
];

/// Vendor office / window presets: (name, modifiers, HID key).
const SHORTCUTS: [(T, u8, u8); 14] = [
    (T::Copy, MOD_CTRL, 0x06),
    (T::Paste, MOD_CTRL, 0x19),
    (T::Cut, MOD_CTRL, 0x1B),
    (T::Undo, MOD_CTRL, 0x1D),
    (T::SelectAll, MOD_CTRL, 0x04),
    (T::Save, MOD_CTRL, 0x16),
    (T::NewDoc, MOD_CTRL, 0x11),
    (T::Print, MOD_CTRL, 0x13),
    (T::Explorer, MOD_WIN, 0x08),
    (T::ShowDesktop, MOD_WIN, 0x07),
    (T::RunDialog, MOD_WIN, 0x15),
    (T::CloseWindow, MOD_ALT, 0x3D),
    (T::Minimize, MOD_WIN, 0x51),
    (T::Maximize, MOD_WIN, 0x52),
];

const MODS: [(u8, &str); 4] = [(MOD_CTRL, "Ctrl"), (MOD_SHIFT, "Shift"), (MOD_ALT, "Alt"), (MOD_WIN, "Win")];

/// "Ctrl + Shift + C".
pub fn combo(mods: u8, usage: u8) -> String {
    let mut parts: Vec<String> = MODS.iter().filter(|(b, _)| mods & b != 0).map(|(_, n)| n.to_string()).collect();
    parts.push(keys::name_of(usage));
    parts.join(" + ")
}

fn mode_label(m: MacroMode) -> String {
    match m {
        MacroMode::Times(n) => fill(tr(T::ModeTimes), &[&n.to_string()]),
        MacroMode::WhileHeld => tr(T::ModeHeld).into(),
        MacroMode::Toggle => tr(T::ModeToggle).into(),
    }
}

/// Human-readable action, e.g. "Copy (Ctrl + C)", "Macro 2 · while held".
pub fn action_label(a: Action) -> String {
    match a {
        Action::Disable => tr(T::ActDisable).into(),
        Action::SwitchEffect => tr(T::ActEffect).into(),
        Action::Dpi(DpiAction::Loop) => tr(T::ActDpiLoop).into(),
        Action::Dpi(DpiAction::Up) => "DPI +".into(),
        Action::Dpi(DpiAction::Down) => "DPI −".into(),
        Action::Mouse(m) => MOUSE_ACTIONS
            .iter()
            .find(|(_, x)| *x == Action::Mouse(m))
            .map_or_else(|| format!("Mouse 0x{m:02X}"), |(t, _)| tr(*t).into()),
        Action::Fire { interval, loops: 0 } => fill(tr(T::ActFireHeld), &[&interval.to_string()]),
        Action::Fire { interval, loops } => fill(tr(T::ActFire), &[&interval.to_string(), &loops.to_string()]),
        Action::Key { mods, usage } => match SHORTCUTS.iter().find(|s| (s.1, s.2) == (mods, usage)) {
            Some((t, ..)) => format!("{} ({})", tr(*t), combo(mods, usage)),
            None => combo(mods, usage),
        },
        Action::Consumer(u) => MEDIA.iter().find(|(_, x)| *x == u).map_or_else(|| format!("Media 0x{u:04X}"), |(t, _)| tr(*t).into()),
        Action::Macro { index, mode } => format!("{} · {}", fill(tr(T::ActMacro), &[&(index + 1).to_string()]), mode_label(mode)),
    }
}

/// Label of a mouse/DPI chip in the picker.
fn mouse_chip_label(t: T, a: Action) -> String {
    match a {
        Action::Dpi(_) => action_label(a),
        _ => tr(t).into(),
    }
}

// ---- buttons page ----------------------------------------------------------

/// Buttons: one panel. On the left the mouse with its buttons numbered,
/// on the right a row per button with the action it has; a click opens
/// the picker.
///
/// ```text
///  +-----------+----------------------------------------+
///  | 3D mouse  | [1] Left button        [Left click  v] |
///  | 1 ─╱▔▔╲─ 2| [2] Right button       [Right click v] |
///  |   ╲__╱    | …                                      |
///  +-----------+----------------------------------------+
/// ```
pub(super) fn buttons_page(f: &mut Frame, v: &View) {
    const BTN_ROW: f32 = 41.0;
    let h = BTN_ROW * v.buttons.len() as f32;

    // The button under the pointer or the keyboard focus, lit on both sides.
    let lit = [f.hot, v.focus.map(|x| x.act)].into_iter().flatten().find_map(|a| match a {
        Act::EditButton(i) => Some(i),
        _ => None,
    });

    let panel = Rect::new(X0, BODY, CW, h);
    f.card(panel);
    f.g.fill(Rect::new(panel.x + ART_W, panel.y, 1.0, h), 0.0, th().line);
    let rows_x = panel.x + ART_W + 1.0;
    for (i, a) in v.buttons.iter().enumerate() {
        let r = Rect::new(rows_x, panel.y + i as f32 * BTN_ROW, panel.w - ART_W - 1.0, BTN_ROW);
        button_row(f, r, i, *a, lit == Some(i));
    }
    // Drawn after the rows: Tab visits the list first. The mesh is built
    // once per mouse; a frame only picks its colors.
    thread_local! {
        static MESH: std::cell::RefCell<Option<(crate::devices::Layout, mesh::Mesh)>> = const { std::cell::RefCell::new(None) };
    }
    let (mx, my) = (panel.x + (ART_W - mesh::WIDTH) / 2.0, panel.y + (h - mesh::HEIGHT) / 2.0);
    MESH.with_borrow_mut(|built| {
        if built.as_ref().is_none_or(|(of, _)| *of != v.layout) {
            *built = Some((v.layout, mesh::build(&v.layout)));
        }
        if let Some((_, m)) = built {
            mesh::draw(f, m, mx, my, lit);
        }
    });

    let y = panel.y + panel.h + PANEL_GAP;
    // Nothing to reset while the layout is the default one.
    let reset = Rect::new(X0, y, 200.0, 28.0);
    if v.buttons == DEFAULT_ACTIONS {
        f.dead_button(reset, tr(T::ResetAll));
    } else {
        f.push_button(Act::ResetButtons, reset, tr(T::ResetAll), false);
    }
    let note = Rect::new(X0 + 216.0, y - 2.0, CW - 216.0, 40.0);
    f.g.text(tr(T::ButtonsNote), note, Size::Caption, Weight::Normal, Align::Wrap, th().muted);
    if !v.notice.is_empty() {
        let r = Rect::new(X0, y + 46.0, CW, 20.0);
        f.g.text(&v.notice, r, Size::Body, Weight::Semibold, Align::Left, th().bad);
    }
}

/// The box of the 3D mouse on the Buttons page.
const ART_W: f32 = mesh::WIDTH;
/// A button row: where the name starts, the select box, and the gap
/// between them (it holds the "changed" dot).
const NAME_X: f32 = PANEL_PAD + 32.0;
const SELECT_W: f32 = 150.0;
const NAME_GAP: f32 = 16.0;
/// Height of a line of a name too long for one.
const NAME_LINE: f32 = 16.0;

/// Room for the button's name in a row `row_w` wide.
fn name_w(row_w: f32) -> f32 {
    row_w - NAME_X - NAME_GAP - SELECT_W - PANEL_PAD
}

/// "[1] Left button            [Left click v]": number badge, name, and
/// the action in a select box; an accent square marks an action changed
/// from the default.
fn button_row(f: &mut Frame, r: Rect, i: usize, a: Action, lit: bool) {
    const SELECT_H: f32 = 28.0;
    const CHANGED: f32 = 3.5;
    let hot = f.button(Act::EditButton(i), r);
    if hot || lit {
        f.g.fill(r, 0.0, th().sel);
    }
    if i > 0 {
        f.g.fill(Rect::new(r.x, r.y, r.w, 1.0), 0.0, th().line);
    }
    let cy = r.y + r.h / 2.0;
    art::marker(f, r.x + PANEL_PAD + 10.0, cy, i, lit);

    let name_x = r.x + NAME_X;
    let select = Rect::new(r.x + r.w - PANEL_PAD - SELECT_W, cy - SELECT_H / 2.0, SELECT_W, SELECT_H);
    // A long name (some languages) takes two lines instead of an ellipsis.
    let (first, second) = name_lines(button_name(i), name_w(r.w), Size::Body);
    let name = match second {
        None => Rect::new(name_x, r.y, name_w(r.w), r.h),
        Some(_) => Rect::new(name_x, cy - NAME_LINE, name_w(r.w), NAME_LINE),
    };
    f.g.text(&first, name, Size::Body, Weight::Semibold, Align::Left, th().text);
    if let Some(rest) = second {
        f.g.text(&rest, Rect::new(name_x, cy, name.w, NAME_LINE), Size::Body, Weight::Semibold, Align::Left, th().text);
    }
    if a != DEFAULT_ACTIONS[i] {
        f.g.dot(select.x - 12.0, cy, CHANGED, th().acc_fill);
    }

    f.g.fill(select, 0.0, th().input);
    f.g.stroke(select, 0.0, 1.0, if hot || lit { th().acc } else { th().line2 });
    let label = Rect::new(select.x + 10.0, select.y, select.w - 40.0, select.h);
    f.g.text(&action_label(a), label, Size::Body, Weight::Normal, Align::Left, th().text);
    glyph(f.g, Glyph::ChevronDown, select.x + select.w - 15.0, cy, th().muted);
}

// ---- macros page -----------------------------------------------------------

/// "12 steps", or `empty` for a macro without any.
fn steps_label(events: &[MacroEvent], empty: &str) -> String {
    if events.is_empty() {
        return empty.into();
    }
    fill(tr(T::Steps), &[&events.len().to_string()])
}

fn event_label(e: &MacroEvent) -> String {
    let (arrow, name) = match e.ev {
        Ev::Key(k, edge) => (edge, keys::name_of(k)),
        Ev::Mods(m, edge) => (edge, keys::mod_name(m).to_string()),
        Ev::Mouse(m, edge) => (edge, format!("Mouse {m}")),
        Ev::MoveX(d) => return format!("X {d:+}"),
        Ev::MoveY(d) => return format!("Y {d:+}"),
        Ev::Wheel(d) => return format!("Wheel {d:+}"),
    };
    let a = if arrow == Edge::Down { "↓" } else { "↑" };
    format!("{a}{name}")
}

/// Buttons that play macro `index`, as names.
fn users_of(v: &View, index: usize) -> Vec<&'static str> {
    v.buttons
        .iter()
        .enumerate()
        .filter(|(_, a)| matches!(a, Action::Macro { index: i, .. } if *i == index))
        .map(|(b, _)| button_name(b))
        .collect()
}

/// Macros: the list on the left, the selected macro on the right.
///
/// ```text
///  +-------------+  +-------------------------------------------+
///  | Macro 1     |  | Macro 2                [Assign to button…] |
///  |▌Macro 2     |  | [Record] [Clear]        Delay [Recorded|…] |
///  | Macro 3     |  | steps …                                    |
///  +-------------+  +-------------------------------------------+
/// ```
pub(super) fn macros_page(f: &mut Frame, v: &View) {
    const LIST_W: f32 = 170.0;
    const LIST_ROW: f32 = 44.0;
    let sel = v.macro_sel;

    // The list: one row per macro, the selected one tinted with a bar.
    let list = Rect::new(X0, BODY, LIST_W, LIST_ROW * MACRO_COUNT as f32);
    f.card(list);
    for i in 0..MACRO_COUNT {
        let r = Rect::new(list.x, list.y + i as f32 * LIST_ROW, LIST_W, LIST_ROW);
        let hot = f.button(Act::MacroSel(i), r);
        if i == sel {
            f.g.fill(Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h - 1.0), 0.0, th().acc_soft);
            f.g.fill(Rect::new(r.x, r.y, 2.0, r.h), 0.0, th().acc_fill);
        } else if hot {
            f.g.fill(Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h - 1.0), 0.0, th().sel);
        }
        if i > 0 {
            f.g.fill(Rect::new(r.x + 1.0, r.y, r.w - 2.0, 1.0), 0.0, th().line);
        }
        let weight = if i == sel { Weight::Semibold } else { Weight::Normal };
        let name = fill(tr(T::ActMacro), &[&(i + 1).to_string()]);
        f.g.text(&name, Rect::new(r.x + PANEL_PAD, r.y + 5.0, r.w - 2.0 * PANEL_PAD, 18.0), Size::Body, weight, Align::Left, th().text);
        let steps = steps_label(&v.macros[i], tr(T::MacroEmptyHint));
        f.g.text(&steps, Rect::new(r.x + PANEL_PAD, r.y + 23.0, r.w - 2.0 * PANEL_PAD, 16.0), Size::Caption, Weight::Normal, Align::Left, th().muted);
    }

    // The editor panel: its header names the macro and assigns it.
    let panel = Rect::new(X0 + LIST_W + PANEL_GAP, BODY, CW - LIST_W - PANEL_GAP, PAGE_H - BODY - 12.0);
    let body = f.panel(panel, &fill(tr(T::ActMacro), &[&(sel + 1).to_string()]));
    let (x, w) = (body.x + PANEL_PAD, body.w - 2.0 * PANEL_PAD);

    let assign_label = tr(T::AssignMacro);
    let assign_w = text_w(assign_label, Size::Body) + 28.0;
    let assign = Rect::new(panel.x + panel.w - PANEL_PAD - assign_w, panel.y + 4.0, assign_w, 28.0);
    if v.macros[sel].is_empty() {
        f.dead_button(assign, assign_label);
    } else {
        f.push_button(Act::AssignMacro, assign, assign_label, false);
    }

    let users = users_of(v, sel);
    let used = if users.is_empty() { tr(T::MacroUnused).to_string() } else { fill(tr(T::MacroUsedBy), &[&users.join(", ")]) };
    f.g.text(&used, Rect::new(x, body.y + PANEL_PAD, w, 18.0), Size::Caption, Weight::Normal, Align::Left, th().muted);

    let y = body.y + PANEL_PAD + 26.0;
    let rec = if v.recording { tr(T::StopRec) } else { tr(T::Record) };
    f.push_button(Act::MacroRecord, Rect::new(x, y, 130.0, 28.0), rec, true);
    let clear = Rect::new(x + 140.0, y, 110.0, 28.0);
    if v.macros[sel].is_empty() {
        f.dead_button(clear, tr(T::Clear));
    } else {
        f.push_button(Act::MacroClear, clear, tr(T::Clear), false);
    }

    // Delay between steps: as recorded, or one fixed value.
    let y = y + 28.0 + 14.0;
    f.caption_in(tr(T::DelayCaption), x, y, w);
    let items: Vec<Segment> = DELAY_CHOICES
        .iter()
        .map(|d| match d {
            DelayMode::Recorded => Segment::named(Act::MacroDelay(*d), tr(T::DelayRecorded), v.delay_mode == *d),
            DelayMode::Fixed(ms) => Segment::new(Act::MacroDelay(*d), format!("{ms} ms"), v.delay_mode == *d),
        })
        .collect();
    f.segments(Rect::new(x, y + 20.0, w, SEGMENT_H), &items);

    // Recorded steps as a wrapped line of tokens.
    let top = y + 20.0 + SEGMENT_H + 14.0;
    let area = Rect::new(x, top, w, panel.y + panel.h - top - PANEL_PAD);
    f.g.fill(area, 0.0, th().input);
    f.g.stroke(area, 0.0, 1.0, if v.recording { th().acc } else { th().line2 });
    let events = &v.macros[sel];
    let mut inner = area.inset(12.0);

    // Recording: accent border and "■ Recording… N steps" with a pulsing mark.
    if v.recording {
        f.g.dot(inner.x + 4.0, inner.y + 9.0, 4.0, Color(th().bad.0, v.pulse));
        let head = fill(tr(T::RecordingSteps), &[&events.len().to_string()]);
        let r = Rect::new(inner.x + 16.0, inner.y, inner.w - 16.0, 18.0);
        f.g.text(&head, r, Size::Body, Weight::Semibold, Align::Left, th().acc);
        inner = Rect::new(inner.x, inner.y + 26.0, inner.w, inner.h - 26.0);
    }

    let text = if !events.is_empty() {
        events.iter().map(|e| format!("{} {}ms", event_label(e), e.delay_ms)).collect::<Vec<_>>().join("   ")
    } else if v.recording {
        tr(T::Recording).into()
    } else {
        tr(T::MacroEmpty).into()
    };
    let color = if events.is_empty() { th().muted } else { th().text };
    // Room for a three-line warning below (long translations).
    const WARN_H: f32 = 48.0;
    f.g.text(&text, Rect::new(inner.x, inner.y, inner.w, inner.h - WARN_H - 2.0), Size::Body, Weight::Normal, Align::Wrap, color);

    // Macros are plain data on disk and in the mouse: warn before passwords end up there.
    let warn = Rect::new(inner.x, inner.y + inner.h - WARN_H, inner.w, WARN_H);
    f.g.text(tr(T::MacroWarn), warn, Size::Caption, Weight::Normal, Align::Wrap, th().warn);
}

/// "Assign macro N to button": one row per button with what it does now.
pub(super) fn assign_dialog(f: &mut Frame, v: &View) {
    const W: f32 = 460.0;
    const ITEM_H: f32 = 40.0;
    let h = 64.0 + ITEM_H * v.buttons.len() as f32 + 76.0;
    f.g.fill(Rect::new(0.0, 0.0, WIDTH, HEIGHT), 0.0, th().scrim);
    let d = Rect::new((WIDTH - W) / 2.0, (HEIGHT - h) / 2.0, W, h);
    f.g.fill(d, RADIUS, th().card_hi);
    f.g.stroke(d, RADIUS, 1.0, th().border);

    let name = fill(tr(T::ActMacro), &[&(v.macro_sel + 1).to_string()]);
    let title = fill(tr(T::AssignMacroTitle), &[&name]);
    f.g.text(&title, Rect::new(d.x + 24.0, d.y + 18.0, d.w - 48.0, 28.0), Size::Subtitle, Weight::Bold, Align::Left, th().text);

    for (i, a) in v.buttons.iter().enumerate() {
        let r = Rect::new(d.x + 16.0, d.y + 60.0 + i as f32 * ITEM_H, d.w - 32.0, ITEM_H - 4.0);
        if f.button(Act::AssignTo(i), r) {
            f.g.fill(r, R_CTRL, th().hover);
        }
        let left = Rect::new(r.x + 12.0, r.y, 170.0, r.h);
        f.g.text(button_name(i), left, Size::Body, Weight::Semibold, Align::Left, th().text);
        let right = Rect::new(r.x + 190.0, r.y, r.w - 202.0, r.h);
        let label = action_label(*a);
        f.g.text(&label, right, Size::Body, Weight::Normal, Align::Left, th().muted);
    }

    let by = d.y + d.h - 24.0 - 36.0;
    f.push_button(Act::CloseDialog, Rect::new(d.x + d.w - 24.0 - 120.0, by, 120.0, 36.0), tr(T::Cancel), false);
}

// ---- assign dialog ---------------------------------------------------------

pub(super) fn picker(f: &mut Frame, v: &View, p: &Picker) {
    f.g.fill(Rect::new(0.0, 0.0, WIDTH, HEIGHT), 0.0, th().scrim);
    let d = Rect::new((WIDTH - 660.0) / 2.0, (HEIGHT - 470.0) / 2.0, 660.0, 470.0);
    f.g.fill(d, RADIUS, th().card_hi);
    f.g.stroke(d, RADIUS, 1.0, th().border);

    let title = fill(tr(T::PickTitle), &[button_name(p.button)]);
    f.g.text(&title, Rect::new(d.x + 24.0, d.y + 18.0, d.w - 48.0, 28.0), Size::Subtitle, Weight::Bold, Align::Left, th().text);

    // Categories: a plain list with a bar, unlike the action tiles.
    for (k, (tab, name)) in TABS.iter().enumerate() {
        let r = Rect::new(d.x + 16.0, d.y + 64.0 + k as f32 * 44.0, 158.0, 38.0);
        category(f, Act::PickTab(*tab), r, tr(*name), Mark::of(p.tab == *tab));
    }
    let sep = d.x + 182.0;
    f.g.line(sep, d.y + 64.0, sep, d.y + d.h - 70.0, 1.0, th().border);

    let area = Rect::new(d.x + 198.0, d.y + 64.0, d.w - 222.0, d.h - 64.0 - 70.0);
    // Highlight the pick, else what the button does now.
    let current = p.chosen.unwrap_or(v.buttons[p.button]);
    match p.tab {
        Tab::Mouse => {
            let items: Vec<(String, Action)> = MOUSE_ACTIONS.iter().map(|(t, a)| (mouse_chip_label(*t, *a), *a)).collect();
            preset_grid(f, area, &items, current);
        }
        Tab::Media => {
            let items: Vec<(String, Action)> = MEDIA.iter().map(|(t, u)| (tr(*t).to_string(), Action::Consumer(*u))).collect();
            preset_grid(f, area, &items, current);
        }
        Tab::Shortcuts => {
            let items: Vec<(String, Action)> =
                SHORTCUTS.iter().map(|(t, m, k)| (tr(*t).to_string(), Action::Key { mods: *m, usage: *k })).collect();
            preset_grid(f, area, &items, current);
        }
        Tab::Keyboard => keyboard_tab(f, area, p),
        Tab::Fire => fire_tab(f, area, p),
        Tab::Macro => macro_tab(f, area, v, p),
        Tab::Other => {
            let items = vec![
                (tr(T::ActDisable).to_string(), Action::Disable),
                (tr(T::ActEffect).to_string(), Action::SwitchEffect),
                (tr(T::RestoreDefault).to_string(), DEFAULT_ACTIONS[p.button]),
            ];
            preset_grid(f, area, &items, current);
        }
    }

    let by = d.y + d.h - 24.0 - 36.0;
    let cancel = Rect::new(d.x + d.w - 24.0 - 120.0, by, 120.0, 36.0);
    f.push_button(Act::PickCancel, cancel, tr(T::Cancel), false);
    let apply = Rect::new(cancel.x - 10.0 - 120.0, by, 120.0, 36.0);
    if p.built().is_some() {
        f.push_button(Act::PickApply, apply, tr(T::Apply), true);
    } else {
        f.dead_button(apply, tr(T::Apply));
    }
}

fn category(f: &mut Frame, act: Act, r: Rect, label: &str, mark: Mark) {
    let selected = mark == Mark::Current;
    let hot = f.button(act, r);
    if selected || hot {
        f.g.fill(r, R_CTRL, if selected { th().accent_soft } else { th().hover });
    }
    if selected {
        f.g.fill(Rect::new(r.x, r.y + 8.0, 3.0, r.h - 16.0), 0.0, th().accent);
    }
    let label_r = Rect::new(r.x + 16.0, r.y, r.w - 20.0, r.h);
    let weight = if selected { Weight::Semibold } else { Weight::Normal };
    f.g.text(label, label_r, Size::Body, weight, Align::Left, if selected { th().text } else { th().muted });
}

/// Two-column grid of presets; a click picks, "Apply" assigns.
fn preset_grid(f: &mut Frame, area: Rect, items: &[(String, Action)], current: Action) {
    let w = (area.w - 10.0) / 2.0;
    let h = if items.len() > 10 { 34.0 } else { 38.0 };
    for (k, (label, a)) in items.iter().enumerate() {
        let (row, col) = ((k / 2) as f32, (k % 2) as f32);
        let r = Rect::new(area.x + col * (w + 10.0), area.y + row * (h + 6.0), w, h);
        f.choice(Act::PickAction(*a), r, label, "", *a == current);
    }
}

fn keyboard_tab(f: &mut Frame, area: Rect, p: &Picker) {
    f.caption(tr(T::Modifiers), area.x, area.y);
    for (k, (bit, name)) in MODS.iter().enumerate() {
        let r = Rect::new(area.x + k as f32 * 92.0, area.y + 22.0, 84.0, 34.0);
        f.choice(Act::PickMod(*bit), r, name, "", p.mods & bit != 0);
    }

    f.caption(tr(T::KeyCaption), area.x, area.y + 74.0);
    let r = Rect::new(area.x, area.y + 96.0, area.w, 52.0);
    let label = match (p.capturing, p.key) {
        (true, _) => tr(T::Waiting).to_string(),
        (false, Some(k)) => keys::name_of(k),
        (false, None) => tr(T::PressKey).to_string(),
    };
    f.choice(Act::PickCapture, r, &label, "", p.capturing);

    if let Some(k) = p.key {
        let preview = Rect::new(area.x, area.y + 162.0, area.w, 30.0);
        f.g.text(&combo(p.mods, k), preview, Size::Subtitle, Weight::Bold, Align::Center, th().accent);
    }
}

fn fire_tab(f: &mut Frame, area: Rect, p: &Picker) {
    f.caption(tr(T::Interval), area.x, area.y);
    for (k, ms) in FIRE_INTERVALS.iter().enumerate() {
        let r = Rect::new(area.x + k as f32 * 78.0, area.y + 22.0, 70.0, 34.0);
        f.choice(Act::PickInterval(*ms), r, &format!("{ms} ms"), "", p.interval == *ms);
    }
    f.caption(tr(T::Repeat), area.x, area.y + 74.0);
    for (k, n) in FIRE_LOOPS.iter().enumerate() {
        let (x, w) = if k == 0 { (area.x, 130.0) } else { (area.x + 138.0 + (k - 1) as f32 * 64.0, 56.0) };
        let label = if *n == 0 { tr(T::ModeHeld).to_string() } else { format!("{n}×") };
        f.choice(Act::PickLoops(*n), Rect::new(x, area.y + 96.0, w, 34.0), &label, "", p.loops == *n);
    }
    if let Some(a) = p.built() {
        let preview = Rect::new(area.x, area.y + 150.0, area.w, 30.0);
        f.g.text(&action_label(a), preview, Size::Large, Weight::Semibold, Align::Left, th().accent);
    }
}

fn macro_tab(f: &mut Frame, area: Rect, v: &View, p: &Picker) {
    f.caption(tr(T::TabMacro), area.x, area.y);
    let w = (area.w - 2.0 * 8.0) / 3.0;
    for i in 0..MACRO_COUNT {
        let (row, col) = ((i / 3) as f32, (i % 3) as f32);
        let r = Rect::new(area.x + col * (w + 8.0), area.y + 22.0 + row * 50.0, w, 44.0);
        let steps = steps_label(&v.macros[i], tr(T::MacroNone));
        f.choice(Act::PickMacro(i), r, &fill(tr(T::ActMacro), &[&(i + 1).to_string()]), &steps, p.macro_index == i);
    }

    let y = area.y + 132.0;
    f.caption(tr(T::Playback), area.x, y);
    for (k, m) in MACRO_MODES.iter().enumerate() {
        let (x, cw, row) = match k {
            0..=2 => (area.x + k as f32 * 64.0, 56.0, 0.0),
            3 => (area.x + 3.0 * 64.0, area.w - 3.0 * 64.0, 0.0),
            _ => (area.x, area.w, 1.0),
        };
        let r = Rect::new(x, y + 22.0 + row * 42.0, cw, 34.0);
        f.choice(Act::PickMode(*m), r, &mode_label(*m), "", p.mode == *m);
    }
    let hint = Rect::new(area.x, y + 110.0, area.w, 20.0);
    f.g.text(tr(T::MacroPickHint), hint, Size::Caption, Weight::Normal, Align::Left, th().muted);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_names_fit_their_column() {
        let room = name_w(CW - ART_W - 1.0);
        for lang in crate::i18n::Lang::ALL {
            crate::i18n::with_lang(lang, || {
                for i in 0..BUTTON_NAMES.len() {
                    // One line, or two for a long name: never an ellipsis.
                    let (first, second) = name_lines(button_name(i), room, Size::Body);
                    for line in [Some(first), second].into_iter().flatten() {
                        assert!(text_w(&line, Size::Body) <= room, "{lang:?}: \"{line}\" is {} wide, room {room}", text_w(&line, Size::Body));
                    }
                }
            });
        }
        // English, the language of most screenshots, needs no second line.
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            for i in 0..BUTTON_NAMES.len() {
                assert_eq!(name_lines(button_name(i), room, Size::Body).1, None, "{}", button_name(i));
            }
        });
    }
}
