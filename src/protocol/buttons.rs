//! Button remapping (cmd 0x04) and macros (cmd 0x0E), see docs/protocol.md §4–5.
//!
//! ```text
//! cmd 0x04, one frame per button:  [7]=button [8..10]=action triple
//! cmd 0x0E, macro chunks:          [5]=(chunk<<4)|total [6]=len+1 [7]=slot [8..]=events
//! event (3 bytes):                 up<<7 | type | delay>>8,  delay & FF,  code
//! ```

use super::{REPORT_LEN, frame};

pub const BUTTON_COUNT: usize = 6;
pub const MACRO_COUNT: usize = 6;
/// 15 chunks x 25 bytes / 3 bytes per event.
pub const MACRO_MAX_EVENTS: usize = 125;
pub const DELAY_MAX_MS: u16 = 0x0FFF;

const CMD_BUTTON: u8 = 0x04;
const CMD_MACRO: u8 = 0x0E;
const CHUNK_BYTES: usize = 25;
const MAX_CHUNKS: usize = 15;

const KIND_DISABLE: u8 = 0x00;
const KIND_MOUSE: u8 = 0x10;
const KIND_FIRE: u8 = 0x30;
const KIND_DPI: u8 = 0x40;
const KIND_EFFECT: u8 = 0x60;
const KIND_KEY: u8 = 0x70;
const KIND_CONSUMER: u8 = 0x80;
const KIND_MACRO: u8 = 0x90;
const SWITCH_EFFECT: u8 = 0x04;

/// Mouse button bits used in actions and macro events.
pub const MB_LEFT: u8 = 0x01;
pub const MB_RIGHT: u8 = 0x02;
pub const MB_MIDDLE: u8 = 0x04;
pub const MB_BACK: u8 = 0x08;
pub const MB_FORWARD: u8 = 0x10;

/// Modifier bits of keyboard actions: Ctrl, Shift, Alt, Win.
pub const MOD_CTRL: u8 = 0x01;
pub const MOD_SHIFT: u8 = 0x02;
pub const MOD_ALT: u8 = 0x04;
pub const MOD_WIN: u8 = 0x08;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpiAction {
    Loop = 1,
    Up = 2,
    Down = 3,
}

/// How a macro bound to a button repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacroMode {
    /// Plays `n` times per press.
    Times(u8),
    /// Repeats while the button is held.
    WhileHeld,
    /// Starts on one press, stops on the next.
    Toggle,
}

impl MacroMode {
    fn wire(self) -> (u8, u8) {
        match self {
            MacroMode::WhileHeld => (1, 0),
            MacroMode::Times(n) => (2, n.max(1)),
            MacroMode::Toggle => (4, 0),
        }
    }

    fn from_wire(kind: u8, count: u8) -> MacroMode {
        match kind {
            1 => MacroMode::WhileHeld,
            4 => MacroMode::Toggle,
            _ => MacroMode::Times(count.max(1)),
        }
    }
}

/// What a mouse button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Disable,
    /// Mouse button bit, e.g. `MB_LEFT`.
    Mouse(u8),
    /// Repeated click: interval in ms (5..255), loops (0 = while held).
    Fire { interval: u8, loops: u8 },
    Dpi(DpiAction),
    SwitchEffect,
    /// Key with modifiers: `mods` = `MOD_*` bits, `usage` = HID keyboard usage.
    Key { mods: u8, usage: u8 },
    /// HID consumer usage (media keys), e.g. 0xCD play/pause.
    Consumer(u16),
    /// Macro by app index 0..MACRO_COUNT; the wire slot is assigned on send.
    Macro { index: usize, mode: MacroMode },
}

/// Factory layout in device order: L, R, M, Back, Forward, DPI.
pub const DEFAULT_ACTIONS: [Action; BUTTON_COUNT] = [
    Action::Mouse(MB_LEFT),
    Action::Mouse(MB_RIGHT),
    Action::Mouse(MB_MIDDLE),
    Action::Mouse(MB_BACK),
    Action::Mouse(MB_FORWARD),
    Action::Dpi(DpiAction::Loop),
];

impl Action {
    /// Wire triple. `slot` is the 1-based macro slot for `Macro` actions.
    pub fn triple(self, slot: u8) -> [u8; 3] {
        match self {
            Action::Disable => [KIND_DISABLE, 0, 0],
            Action::Mouse(m) => [KIND_MOUSE, m, 0],
            Action::Fire { interval, loops } => [KIND_FIRE, interval, loops],
            Action::Dpi(d) => [KIND_DPI, d as u8, 0],
            Action::SwitchEffect => [KIND_EFFECT, SWITCH_EFFECT, 0],
            Action::Key { mods, usage } => [KIND_KEY, mods, usage],
            Action::Consumer(u) => [KIND_CONSUMER, u as u8, (u >> 8) as u8],
            Action::Macro { mode, .. } => {
                let (kind, count) = mode.wire();
                [KIND_MACRO, (kind << 4) | (slot & 0x0F), count]
            }
        }
    }

    /// Storage form: the macro "slot" nibble holds `index + 1`.
    pub fn to_stored(self) -> [u8; 3] {
        let slot = match self {
            Action::Macro { index, .. } => index as u8 + 1,
            _ => 0,
        };
        self.triple(slot)
    }

    /// Parses a stored or vendor triple. `None` for unknown kinds.
    pub fn from_triple(t: [u8; 3]) -> Option<Action> {
        let a = match t[0] {
            KIND_DISABLE => Action::Disable,
            KIND_MOUSE => Action::Mouse(t[1]),
            KIND_FIRE => Action::Fire { interval: t[1], loops: t[2] },
            KIND_DPI => Action::Dpi(match t[1] {
                2 => DpiAction::Up,
                3 => DpiAction::Down,
                _ => DpiAction::Loop,
            }),
            KIND_EFFECT => Action::SwitchEffect,
            KIND_KEY => Action::Key { mods: t[1], usage: t[2] },
            KIND_CONSUMER => Action::Consumer(u16::from_le_bytes([t[1], t[2]])),
            KIND_MACRO => {
                let index = ((t[1] & 0x0F) as usize).checked_sub(1).filter(|&i| i < MACRO_COUNT)?;
                Action::Macro { index, mode: MacroMode::from_wire(t[1] >> 4, t[2]) }
            }
            _ => return None,
        };
        Some(a)
    }
}

// ---- macros ----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ev {
    /// HID keyboard usage.
    Key(u8, Edge),
    /// Modifier bit mask of usages E0..E7 (01 LCtrl, 02 LShift, 04 LAlt, 08 LWin…).
    Mods(u8, Edge),
    Mouse(u8, Edge),
    MoveX(i8),
    MoveY(i8),
    Wheel(i8),
}

/// One macro step; `delay_ms` is the pause after it (0..=4095).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MacroEvent {
    pub ev: Ev,
    pub delay_ms: u16,
}

impl MacroEvent {
    pub fn encode(&self) -> [u8; 3] {
        let (kind, up, code) = match self.ev {
            Ev::Mouse(m, e) => (0x10, e == Edge::Up, m),
            Ev::Mods(m, e) => (0x20, e == Edge::Up, m),
            Ev::Key(k, e) => (0x30, e == Edge::Up, k),
            Ev::MoveX(d) => (0x40, false, d as u8),
            Ev::MoveY(d) => (0x50, false, d as u8),
            Ev::Wheel(d) => (0x60, false, d as u8),
        };
        let delay = self.delay_ms.min(DELAY_MAX_MS);
        [((up as u8) << 7) | kind | (delay >> 8) as u8, delay as u8, code]
    }

    pub fn decode(b: [u8; 3]) -> Option<MacroEvent> {
        let edge = if b[0] & 0x80 != 0 { Edge::Up } else { Edge::Down };
        let ev = match b[0] & 0x70 {
            0x10 => Ev::Mouse(b[2], edge),
            0x20 => Ev::Mods(b[2], edge),
            0x30 => Ev::Key(b[2], edge),
            0x40 => Ev::MoveX(b[2] as i8),
            0x50 => Ev::MoveY(b[2] as i8),
            0x60 => Ev::Wheel(b[2] as i8),
            _ => return None,
        };
        let delay_ms = (((b[0] & 0x0F) as u16) << 8) | b[1] as u16;
        Some(MacroEvent { ev, delay_ms })
    }
}

/// Fire-key interval range accepted by the vendor dialog.
pub const FIRE_MIN_MS: u8 = 5;
/// Highest HID keyboard usage the vendor key table maps (F24).
const KEY_USAGE_MAX: u8 = 0x91;
const MOUSE_MASK: u8 = MB_LEFT | MB_RIGHT | MB_MIDDLE | MB_BACK | MB_FORWARD;

impl Action {
    /// Values a hand-edited config could carry, made safe for the firmware:
    /// fire interval clamped, unknown keys or mouse bits become `Disable`.
    pub fn sanitized(self) -> Action {
        match self {
            Action::Fire { interval, loops } => Action::Fire { interval: interval.max(FIRE_MIN_MS), loops },
            Action::Key { usage, .. } if usage == 0 || usage > KEY_USAGE_MAX => Action::Disable,
            Action::Key { mods, usage } => Action::Key { mods: mods & 0x0F, usage },
            Action::Mouse(m) if m == 0 || m & !MOUSE_MASK != 0 => Action::Disable,
            a => a,
        }
    }
}

/// Every press gets a release: keys still held at the end (e.g. Alt after
/// Alt+Tab left the window) are released with 0 ms delay, and the list is
/// cut so those releases fit under `MACRO_MAX_EVENTS`.
pub fn balance(mut events: Vec<MacroEvent>) -> Vec<MacroEvent> {
    loop {
        let held = held_presses(&events);
        if events.len() + held.len() <= MACRO_MAX_EVENTS {
            events.extend(held.into_iter().map(|ev| MacroEvent { ev, delay_ms: 0 }));
            return events;
        }
        events.truncate(MACRO_MAX_EVENTS - held.len().min(MACRO_MAX_EVENTS));
    }
}

/// Release events for presses that have no release yet, in press order.
fn held_presses(events: &[MacroEvent]) -> Vec<Ev> {
    let mut held: Vec<Ev> = Vec::new();
    for e in events {
        let (release, down) = match e.ev {
            Ev::Key(k, d) => (Ev::Key(k, Edge::Up), d == Edge::Down),
            Ev::Mods(m, d) => (Ev::Mods(m, Edge::Up), d == Edge::Down),
            Ev::Mouse(m, d) => (Ev::Mouse(m, Edge::Up), d == Edge::Down),
            _ => continue,
        };
        held.retain(|h| *h != release);
        if down {
            held.push(release);
        }
    }
    held
}

/// Macro slot per button: distinct macros get slots 1, 2, … in button order.
fn assign_slots(actions: &[Action; BUTTON_COUNT]) -> [Option<(u8, usize)>; BUTTON_COUNT] {
    let mut seen: Vec<usize> = Vec::new();
    let mut out = [None; BUTTON_COUNT];
    for (b, a) in actions.iter().enumerate() {
        let Action::Macro { index, .. } = *a else {
            continue;
        };
        let pos = seen.iter().position(|&i| i == index).unwrap_or_else(|| {
            seen.push(index);
            seen.len() - 1
        });
        out[b] = Some((pos as u8 + 1, index));
    }
    out
}

fn macro_chunks(events: &[MacroEvent], slot: u8, seq: &mut impl FnMut() -> u8) -> Vec<[u8; REPORT_LEN]> {
    let bytes: Vec<u8> = events.iter().take(MACRO_MAX_EVENTS).flat_map(|e| e.encode()).collect();
    let total = bytes.len().div_ceil(CHUNK_BYTES).clamp(1, MAX_CHUNKS);

    (0..total)
        .map(|k| {
            let part = bytes.get(k * CHUNK_BYTES..).unwrap_or(&[]);
            let part = &part[..part.len().min(CHUNK_BYTES)];
            let mut d = vec![slot];
            d.extend_from_slice(part);

            let mut f = frame(CMD_MACRO, seq(), &d);
            f[5] = ((k as u8) << 4) | total as u8;
            f
        })
        .collect()
}

/// All frames that program the buttons: one cmd 0x04 per button, then the
/// cmd 0x0E chunks of every bound macro.
pub fn button_frames(actions: &[Action; BUTTON_COUNT], macros: &[Vec<MacroEvent>], mut seq: impl FnMut() -> u8) -> Vec<[u8; REPORT_LEN]> {
    let slots = assign_slots(actions);
    let mut out = Vec::new();

    for (b, a) in actions.iter().enumerate() {
        let slot = slots[b].map_or(0, |(s, _)| s);
        let t = a.triple(slot);
        out.push(frame(CMD_BUTTON, seq(), &[b as u8, t[0], t[1], t[2]]));
    }

    let mut sent: Vec<u8> = Vec::new();
    for (slot, index) in slots.into_iter().flatten() {
        if sent.contains(&slot) {
            continue;
        }
        sent.push(slot);
        let events = macros.get(index).map(Vec::as_slice).unwrap_or(&[]);
        out.extend(macro_chunks(events, slot, &mut seq));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero() -> impl FnMut() -> u8 {
        || 0
    }

    #[test]
    fn spec_example_macro() {
        // docs/protocol.md §5: slot 1, "A down, 172 ms, A up, 47 ms", once.
        let mut actions = DEFAULT_ACTIONS;
        actions[3] = Action::Macro { index: 0, mode: MacroMode::Times(1) };
        let a = 0x04;
        let m = vec![MacroEvent { ev: Ev::Key(a, Edge::Down), delay_ms: 172 }, MacroEvent { ev: Ev::Key(a, Edge::Up), delay_ms: 47 }];
        let frames = button_frames(&actions, &[m], zero());

        assert_eq!(frames.len(), BUTTON_COUNT + 1);
        assert_eq!(&frames[3][2..11], &[0x04, 0, 0, 0x01, 0x04, 0x03, 0x90, 0x21, 0x01]);
        assert_eq!(&frames[6][..14], &[0x0C, 0x01, 0x0E, 0, 0, 0x01, 0x07, 0x01, 0x30, 0xAC, 0x04, 0xB0, 0x2F, 0x04]);
    }

    #[test]
    fn default_triples() {
        let t: Vec<[u8; 3]> = DEFAULT_ACTIONS.iter().map(|a| a.triple(0)).collect();
        assert_eq!(t, vec![[0x10, 1, 0], [0x10, 2, 0], [0x10, 4, 0], [0x10, 8, 0], [0x10, 0x10, 0], [0x40, 1, 0]]);
    }

    #[test]
    fn long_macro_is_chunked() {
        let ev = MacroEvent { ev: Ev::Key(0x04, Edge::Down), delay_ms: 10 };
        let mut seq = zero();
        let f = macro_chunks(&vec![ev; 10], 2, &mut seq);
        assert_eq!(f.len(), 2);
        assert_eq!((f[0][5], f[0][6], f[0][7]), (0x02, 26, 2));
        assert_eq!((f[1][5], f[1][6]), (0x12, 6));
    }

    #[test]
    fn shared_macro_gets_one_slot() {
        let mut actions = DEFAULT_ACTIONS;
        actions[3] = Action::Macro { index: 4, mode: MacroMode::Toggle };
        actions[4] = Action::Macro { index: 4, mode: MacroMode::WhileHeld };
        let frames = button_frames(&actions, &vec![Vec::new(); MACRO_COUNT], zero());
        assert_eq!(frames.len(), BUTTON_COUNT + 1);
        assert_eq!((frames[3][9], frames[4][9]), (0x41, 0x11));
    }

    #[test]
    fn stored_triples_roundtrip() {
        let all = [
            Action::Disable,
            Action::Fire { interval: 50, loops: 3 },
            Action::Dpi(DpiAction::Down),
            Action::SwitchEffect,
            Action::Key { mods: MOD_CTRL, usage: 0x06 },
            Action::Consumer(0x0192),
            Action::Macro { index: 5, mode: MacroMode::Times(3) },
        ];
        for a in all {
            assert_eq!(Action::from_triple(a.to_stored()), Some(a));
        }
    }

    fn down(k: u8) -> MacroEvent {
        MacroEvent { ev: Ev::Key(k, Edge::Down), delay_ms: 5 }
    }

    #[test]
    fn balance_releases_held_keys() {
        // Alt held (Alt+Tab left the window), A pressed and released.
        let alt = MacroEvent { ev: Ev::Mods(0x04, Edge::Down), delay_ms: 5 };
        let a_up = MacroEvent { ev: Ev::Key(0x04, Edge::Up), delay_ms: 5 };
        let out = balance(vec![alt, down(0x04), a_up]);
        assert_eq!(out.len(), 4);
        assert_eq!(out[3], MacroEvent { ev: Ev::Mods(0x04, Edge::Up), delay_ms: 0 });
    }

    #[test]
    fn balance_keeps_room_under_cap() {
        let full: Vec<MacroEvent> = (0..MACRO_MAX_EVENTS).map(|i| down(0x04 + (i % 20) as u8)).collect();
        let out = balance(full);
        assert!(out.len() <= MACRO_MAX_EVENTS);
        assert!(held_presses(&out).is_empty());
    }

    #[test]
    fn sanitize_bounds_values() {
        assert_eq!(Action::Fire { interval: 0, loops: 1 }.sanitized(), Action::Fire { interval: FIRE_MIN_MS, loops: 1 });
        assert_eq!(Action::Key { mods: 0xFF, usage: 0xF0 }.sanitized(), Action::Disable);
        assert_eq!(Action::Mouse(0x40).sanitized(), Action::Disable);
        assert_eq!(Action::Mouse(MB_BACK).sanitized(), Action::Mouse(MB_BACK));
    }

    #[test]
    fn event_roundtrip_and_delay_clamp() {
        let e = MacroEvent { ev: Ev::MoveY(-5), delay_ms: 9999 };
        let d = MacroEvent::decode(e.encode()).unwrap();
        assert_eq!(d, MacroEvent { ev: Ev::MoveY(-5), delay_ms: DELAY_MAX_MS });
    }
}
