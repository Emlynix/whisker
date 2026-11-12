//! Portable half of the window shell: the app's hooks, key and modifier
//! events, and the pointer logic that turns presses on the hits of the
//! last frame into `Act`s. The platform window feeds it pointer
//! positions in DIPs and carries out what it returns.

use crate::gfx::Gfx;
use crate::tray::TrayAction;
use crate::view::{Act, Hit, Kind};

/// Keyboard event for the app (shortcut capture, macro recording).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// Windows virtual-key code; other backends translate to it.
    pub vk: u16,
    pub down: bool,
    /// Right Ctrl/Alt and other extended keys.
    pub extended: bool,
    /// Auto-repeat of a held key.
    pub repeat: bool,
}

/// Which file dialog `window::ask_file` shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileAsk {
    Open,
    Save,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Move,
    Done,
}

/// Callbacks into the app. Plain fns: the app keeps its own state.
pub struct Hooks {
    pub draw: fn(&mut Gfx, Option<Act>) -> Vec<Hit>,
    pub act: fn(Act),
    pub drag: fn(Act, usize, Phase),
    /// Mouse wheel at a point (DIPs): notches, positive = away from the user.
    pub wheel: fn(f32, f32, i32),
    pub timer: fn(usize),
    pub tray: fn(TrayAction),
    pub hidden: fn(),
    /// OS settings changed (e.g. light/dark mode).
    pub settings: fn(),
    /// Returns true when the app consumed the key.
    pub key: fn(KeyEvent) -> bool,
    /// The window lost keyboard focus.
    pub focus_lost: fn(),
}

/// Modifier keys held right now (for shortcut capture).
#[derive(Clone, Copy, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub win: bool,
}

/// Result of a pointer move.
pub enum Moved {
    /// A slider drag continues: call `Hooks::drag` with `Phase::Move`.
    Drag(Act, usize),
    /// The hovered element changed.
    Redraw,
    Nothing,
}

/// Result of releasing the button.
pub enum Released {
    /// A slider drag ended: release capture, call `Hooks::drag` with `Phase::Done`.
    Drag(Act, usize),
    /// Pressed and released on the same element.
    Click(Act),
    Nothing,
}

/// Pointer state over the hits of the last frame.
#[derive(Default)]
pub struct Pointer {
    hits: Vec<Hit>,
    hot: Option<Act>,
    pressed: Option<Act>,
    drag: Option<Hit>,
}

impl Pointer {
    pub fn set_hits(&mut self, hits: Vec<Hit>) {
        self.hits = hits;
    }

    /// Hovered element, drawn highlighted.
    pub fn hot(&self) -> Option<Act> {
        self.hot
    }

    /// Something is under or held by the pointer (hand cursor).
    pub fn busy(&self) -> bool {
        self.hot.is_some() || self.drag.is_some()
    }

    pub fn moved(&mut self, x: f32, y: f32) -> Moved {
        if let Some(h) = self.drag {
            return Moved::Drag(h.act, slider_value(&h, x));
        }
        let hot = self.control_at(x, y).map(|h| h.act);
        if hot == self.hot {
            return Moved::Nothing;
        }
        self.hot = hot;
        Moved::Redraw
    }

    /// Button down; `Some` starts a slider drag (capture the pointer, call
    /// `Hooks::drag` with `Phase::Move`).
    pub fn down(&mut self, x: f32, y: f32) -> Option<(Act, usize)> {
        let h = self.control_at(x, y)?;
        if let Kind::Slider { .. } = h.kind {
            self.drag = Some(h);
            return Some((h.act, slider_value(&h, x)));
        }
        self.pressed = Some(h.act);
        None
    }

    pub fn up(&mut self, x: f32, y: f32) -> Released {
        if let Some(h) = self.drag.take() {
            return Released::Drag(h.act, slider_value(&h, x));
        }
        let pressed = self.pressed.take();
        let under = self.control_at(x, y).map(|h| h.act);
        match pressed {
            Some(a) if pressed == under => Released::Click(a),
            _ => Released::Nothing,
        }
    }

    /// The pointer left the window.
    pub fn leave(&mut self) {
        self.hot = None;
    }

    /// The point is on the empty part of the title bar: a press there
    /// moves the window (the platform window does that, not the app).
    pub fn caption_at(&self, x: f32, y: f32) -> bool {
        self.hit_at(x, y).is_some_and(|h| matches!(h.kind, Kind::Caption))
    }

    /// Something to press at the point; the caption is not.
    fn control_at(&self, x: f32, y: f32) -> Option<Hit> {
        self.hit_at(x, y).filter(|h| !matches!(h.kind, Kind::Caption))
    }

    fn hit_at(&self, x: f32, y: f32) -> Option<Hit> {
        // Last pushed is drawn on top, so search from the end.
        self.hits.iter().rev().find(|h| h.rect.contains(x, y)).copied()
    }
}

fn slider_value(h: &Hit, x: f32) -> usize {
    let Kind::Slider { x0, x1, steps } = h.kind else {
        return 0;
    };
    let t = ((x - x0) / (x1 - x0)).clamp(0.0, 1.0);
    (t * (steps - 1) as f32).round() as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::Rect;

    /// A title bar with one button on its right end.
    fn title_bar() -> Pointer {
        let mut p = Pointer::default();
        p.set_hits(vec![
            Hit { act: Act::Caption, rect: Rect::new(0.0, 0.0, 800.0, 32.0), kind: Kind::Caption },
            Hit { act: Act::WinClose, rect: Rect::new(760.0, 0.0, 40.0, 32.0), kind: Kind::Button },
        ]);
        p
    }

    #[test]
    fn the_empty_title_bar_drags_and_its_buttons_click() {
        let mut p = title_bar();
        assert!(p.caption_at(300.0, 10.0));
        assert!(!p.caption_at(780.0, 10.0), "a button on the bar is no caption");
        assert!(!p.caption_at(300.0, 100.0), "below the bar");

        // The caption itself is never hovered, pressed or clicked.
        assert!(matches!(p.moved(300.0, 10.0), Moved::Nothing));
        assert!(!p.busy());
        assert!(p.down(300.0, 10.0).is_none());
        assert!(matches!(p.up(300.0, 10.0), Released::Nothing));

        assert!(p.down(780.0, 10.0).is_none());
        assert!(matches!(p.up(780.0, 10.0), Released::Click(Act::WinClose)));
    }
}
