//! Controller: the only place that connects the UI with the services.
//!
//! ```text
//!   window (Win32 shell) --Act--> Controller --> MouseService --> HidLink
//!        ^                            |-------> Stats (Raw Input)
//!        +------ view::draw(View) ----+-------> store / autostart / install
//! ```
//!
//! Lives on the UI thread in a thread-local; other threads reach it with
//! `window::post(.. with_ctrl(..))`.

use crate::autostart::{self, Autostart};
use crate::devices::{self, CAPS_NONE, Caps, Family, Link, Match, Support};
use crate::drivers::{Ability, Ident};
use crate::mouse::Target;
use crate::hid_link::{self, DeviceInfo};
use crate::gfx::Gfx;
use crate::i18n::{self, Lang, T, fill, tr};
use crate::install;
use crate::mouse::{Command, Event, MouseService};
use crate::keys;
use crate::protocol::{
    self, Action, BREATHING_MAX, DEFAULT_ACTIONS, DELAY_MAX_MS, Edge, Effect, Ev, MACRO_MAX_EVENTS, MB_LEFT, MOD_ALT,
    MOD_CTRL, MOD_SHIFT, MOD_WIN, MacroEvent, PollingRate, STAGE_COUNT, Status,
};
use crate::stats::{self, Period, Stats};
use crate::theme::{self, THEMES};
use crate::store::{self, AppConfig};
use crate::tray::{Level, TrayAction};
use crate::view::{self, Act, Badge, BasicView, Charge, DelayMode, Detect, DetectRow, DeviceView, Hit, Picker, StageView, View};
use crate::window::{self, Hooks, KeyEvent, Phase};
use std::cell::RefCell;
use std::time::Instant;

const TIMER_REFRESH: usize = 1;
const TIMER_SAVE: usize = 2;
const REFRESH_MS: u32 = 1000;
const SAVE_MS: u32 = 60_000;
const CHART_DAYS: u64 = 7;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StartMode {
    Visible,
    Hidden,
}

thread_local! {
    static CTRL: RefCell<Option<Controller>> = const { RefCell::new(None) };
}

/// Runs `f` on the controller; skipped if it is busy (re-entrant message).
fn with_ctrl<R>(f: impl FnOnce(&mut Controller) -> R) -> Option<R> {
    CTRL.with(|c| c.try_borrow_mut().ok()?.as_mut().map(f))
}

/// Runs `f` on the UI thread from any thread.
fn post(f: impl FnOnce(&mut Controller) + Send + 'static) {
    window::post(move || {
        with_ctrl(f);
    });
}

struct Controller {
    view: View,
    cfg: AppConfig,
    stats: Stats,
    mouse: MouseService,
    status: Option<Status>,
    link: T,
    level: Level,
    period: Period,
    /// Uninstalled: never write data again.
    removed: bool,
    /// Time of the last recorded macro step.
    last_step: Option<Instant>,
    /// Identity of the opened configuration interface.
    device_info: Option<DeviceInfo>,
    /// Mice found by the last detection.
    mice: Vec<DeviceInfo>,
    /// Identity reported by a driver-based mouse.
    ident: Option<Ident>,
    /// A driver-based mouse answered the last poll.
    basic_online: bool,
    /// Driver the mouse service was last pointed at.
    target: Target,
}

pub fn run(mode: StartMode) {
    let hooks = Hooks {
        draw: hook_draw,
        act: hook_act,
        drag: hook_drag,
        timer: hook_timer,
        tray: hook_tray,
        hidden: hook_hidden,
        settings: hook_settings,
        key: hook_key,
        focus_lost: hook_focus_lost,
    };
    if !window::create(crate::APP_NAME, view::WIDTH, view::HEIGHT, hooks) {
        return;
    }

    let cfg = store::load_config();
    apply_look(&cfg);
    let first_run = !cfg.setup_asked && !install::is_installed_copy();
    let target = target_of(cfg.device.as_deref());
    let ctrl = Controller {
        view: initial_view(&cfg, first_run),
        cfg,
        stats: Stats::start(),
        mouse: MouseService::start(target.clone(), |ev| post(move |c| c.on_mouse(ev))),
        status: None,
        link: T::LinkSearching,
        level: Level::Offline,
        period: Period::Today,
        removed: false,
        last_step: None,
        device_info: None,
        mice: Vec::new(),
        ident: None,
        basic_online: false,
        target: target.clone(),
    };
    ctrl.stats.set_vendor(stats_vendor(ctrl.cfg.device.as_deref()));
    CTRL.with(|c| *c.borrow_mut() = Some(ctrl));
    with_ctrl(|c| {
        c.sync_stages();
        c.refresh_stats();
    });

    // Which mice are connected; asks the user on first run.
    std::thread::spawn(|| {
        let mice = hid_link::list_mice();
        post(move |c| c.on_mice(mice));
    });

    crate::single::listen(|| post(|c| c.show()), || window::post(window::quit));
    window::set_timer(TIMER_SAVE, SAVE_MS);

    if mode == StartMode::Visible || first_run {
        with_ctrl(|c| c.show());
    }
    window::run();

    with_ctrl(|c| {
        if !c.removed {
            c.stats.save();
        }
    });
}

fn initial_view(cfg: &AppConfig, ask_setup: bool) -> View {
    View {
        page: 0,
        period: 0,
        connected: false,
        link: tr(T::LinkSearching).into(),
        battery: 0,
        charge: Charge::Discharging,
        battery_note: String::new(),
        stages: Vec::new(),
        active: cfg.dpi.active,
        steps: protocol::dpi_steps(),
        polling: rate_index(cfg.polling),
        measured: "—".into(),
        today_clicks: "0".into(),
        today_distance: "0 m".into(),
        tiles: Vec::new(),
        chart: Vec::new(),
        chart_caption: String::new(),
        autostart: autostart::is_enabled(),
        installed: install::is_installed_copy(),
        install_dir: install::install_dir().map(|d| d.display().to_string()).unwrap_or_default(),
        error: String::new(),
        data_dir: store::data_dir().map(|d| d.display().to_string()).unwrap_or_default(),
        ask_setup,
        setup_autostart: true,
        theme_pick: match cfg.theme {
            theme::Choice::System => 0,
            theme::Choice::Fixed(i) => i + 1,
        },
        lang_pick: cfg.lang.and_then(|l| Lang::ALL.iter().position(|&x| x == l)).map_or(0, |i| i + 1),
        lighting: cfg.lighting.clone(),
        dpi_colors: cfg.dpi_colors,
        debounce: cfg.debounce,
        lod: cfg.lod,
        power: cfg.power,
        buttons: cfg.buttons,
        macros: cfg.macros.clone(),
        picker: None,
        macro_sel: 0,
        recording: false,
        delay_mode: DelayMode::Recorded,
        notice: String::new(),
        caps: caps_of(cfg.device.as_deref()),
        device_name: device_name(cfg.device.as_deref()),
        device: DeviceView::default(),
        detect: None,
        basic: BasicView::default(),
        // Until a reading arrives: only mice that can report a battery show one.
        has_battery: caps_of(cfg.device.as_deref()).battery,
    }
}

/// Applies the configured theme and language to the UI.
fn apply_look(cfg: &AppConfig) {
    theme::apply(cfg.theme);
    i18n::set_lang(cfg.lang.unwrap_or_else(i18n::system_lang));
    window::set_dark_title(theme::current().dark);
}

/// Pages to show for the configured mouse. Until one is chosen, show all.
fn caps_of(device: Option<&str>) -> Caps {
    match device {
        None => devices::MODELS[0].caps,
        Some(id) => devices::by_id(id).map_or(CAPS_NONE, |m| m.caps),
    }
}

/// Driver for the configured mouse; until one is chosen, the Lionel driver.
fn target_of(device: Option<&str>) -> Target {
    match device {
        None => Target::Lionel,
        Some(id) => devices::by_id(id).map_or(Target::Nothing, |m| m.family.target()),
    }
}

/// Whether an event can come from the driver now in use. Events queued
/// by the previous driver arrive after a switch and must not overwrite
/// the new mouse's state.
fn belongs_to(ev: &Event, t: &Target) -> bool {
    let driver = matches!(t, Target::Hidpp | Target::Razer | Target::Eternico | Target::Sinowealth | Target::SteelSeries | Target::OemHid);
    match ev {
        Event::Status(_) => *t == Target::Lionel,
        Event::Device(_) => *t == Target::Lionel || driver,
        Event::Ident(..) => driver,
        Event::Basic(_) => driver || matches!(t, Target::Bluetooth(_)),
        Event::Offline | Event::Absent => *t != Target::Nothing,
    }
}

/// Clears what the previous mouse reported; flags follow the new one's
/// capabilities until it reports itself.
fn forget_mouse(v: &mut View, caps: Caps) {
    v.basic = BasicView::default();
    v.connected = false;
    v.battery = 0;
    v.charge = Charge::Discharging;
    v.has_battery = caps.battery;
    v.link = tr(T::LinkSearching).into();
}

fn support_badge(s: Support) -> (&'static str, Badge) {
    match s {
        Support::Full => (tr(T::BadgeFull), Badge::Good),
        Support::Experimental => (tr(T::BadgeExperimental), Badge::Warn),
        Support::Basic => (tr(T::BadgeUnknown), Badge::Muted),
    }
}

/// Statistics count the configured mouse only; "other mouse" counts all mice.
fn stats_vendor(device: Option<&str>) -> Option<u16> {
    match device {
        Some(id) => devices::by_id(id).map(|m| m.vid),
        None => Some(devices::MODELS[0].vid),
    }
}

fn device_name(device: Option<&str>) -> String {
    match device.and_then(devices::by_id) {
        Some(m) => m.full_name(),
        None if device.is_some() => tr(T::GenericMouse).into(),
        None => devices::MODELS[0].full_name(),
    }
}

/// Dialog rows for detected mice; an unknown one maps to "statistics only".
fn detect_rows(mice: &[DeviceInfo]) -> Vec<DetectRow> {
    mice.iter()
        .map(|d| {
            let usb = format!("{:04X}:{:04X}", d.vid, d.pid);
            let sub = if d.product.is_empty() { usb } else { format!("{usb} · {}", d.product) };
            let (title, badge, kind, choice) = match devices::match_device(d) {
                Match::Known(m) => {
                    let (b, k) = support_badge(m.support);
                    // Vendor-wide entries (any product id) show the product name.
                    let title = if m.pids.is_empty() && !d.product.is_empty() { format!("{} {}", m.vendor, d.product) } else { m.full_name() };
                    (title, b.to_string(), k, m.id.to_string())
                }
                Match::Guess(m) => {
                    let title = if d.product.is_empty() { tr(T::UnknownMouse).to_string() } else { d.product.clone() };
                    (title, fill(tr(T::BadgeGuess), &[&m.full_name()]), Badge::Warn, m.id.to_string())
                }
                Match::Unknown => {
                    let title = if d.product.is_empty() { tr(T::UnknownMouse).to_string() } else { d.product.clone() };
                    (title, tr(T::BadgeUnknown).to_string(), Badge::Muted, store::GENERIC_DEVICE.to_string())
                }
            };
            DetectRow { title, sub, badge, kind, choice }
        })
        .collect()
}

/// Manual choice: every catalogue model plus "other mouse".
fn model_rows() -> Vec<DetectRow> {
    let mut rows: Vec<DetectRow> = devices::MODELS
        .iter()
        .map(|m| {
            let (badge, kind) = support_badge(m.support);
            let pids = m.pids.iter().map(|(p, _)| format!("{p:04X}")).collect::<Vec<_>>().join("/");
            let sub = format!("{:04X}:{}", m.vid, if pids.is_empty() { "*".to_string() } else { pids });
            DetectRow { title: m.full_name(), sub, badge: badge.into(), kind, choice: m.id.into() }
        })
        .collect();
    rows.push(DetectRow {
        title: tr(T::GenericMouse).into(),
        sub: String::new(),
        badge: tr(T::BadgeUnknown).into(),
        kind: Badge::Muted,
        choice: store::GENERIC_DEVICE.into(),
    });
    rows
}

fn rate_index(r: PollingRate) -> usize {
    PollingRate::ALL.iter().position(|&x| x == r).unwrap_or(0)
}

// ---- window hooks ----------------------------------------------------------

fn hook_draw(g: &mut Gfx, hot: Option<Act>) -> Vec<Hit> {
    with_ctrl(|c| view::draw(g, &c.view, hot)).unwrap_or_default()
}

fn hook_act(a: Act) {
    with_ctrl(|c| c.on_act(a));
}

fn hook_drag(a: Act, value: usize, phase: Phase) {
    if a == Act::BasicDpi {
        with_ctrl(|c| c.drag_basic_dpi(value, phase));
        return;
    }
    let Act::Slider(i) = a else {
        return;
    };
    with_ctrl(|c| match phase {
        Phase::Move => c.preview_stage(i, value),
        Phase::Done => c.set_stage(i, value),
    });
}

fn hook_timer(id: usize) {
    with_ctrl(|c| match id {
        TIMER_REFRESH => c.refresh_stats(),
        TIMER_SAVE if !c.removed => c.stats.save(),
        _ => {}
    });
}

fn hook_tray(a: TrayAction) {
    match a {
        TrayAction::Open => {
            with_ctrl(|c| c.show());
        }
        TrayAction::Quit => window::quit(),
    }
}

fn hook_hidden() {
    with_ctrl(|c| c.on_hidden());
}

/// Macro modifier bits (left/right) -> shortcut `MOD_*` bit.
fn shortcut_mod(bits: u8) -> u8 {
    match bits {
        0x01 | 0x10 => MOD_CTRL,
        0x02 | 0x20 => MOD_SHIFT,
        0x04 | 0x40 => MOD_ALT,
        _ => MOD_WIN,
    }
}

fn hook_focus_lost() {
    with_ctrl(|c| c.on_focus_lost());
}

fn hook_key(k: KeyEvent) -> bool {
    with_ctrl(|c| c.on_key(k)).unwrap_or(false)
}

/// Windows light/dark switch matters when the theme follows the system.
fn hook_settings() {
    with_ctrl(|c| {
        apply_look(&c.cfg);
        window::invalidate();
    });
}

impl Controller {
    // ---- window ----

    fn show(&mut self) {
        window::show();
        self.mouse.send(Command::Visible(true));
        self.mouse.send(Command::RefreshNow);
        self.refresh_stats();
        window::set_timer(TIMER_REFRESH, REFRESH_MS);
    }

    /// Hidden window: slow polling, no UI refresh timer.
    fn on_hidden(&mut self) {
        self.on_focus_lost();
        window::kill_timer(TIMER_REFRESH);
        self.mouse.send(Command::Visible(false));
        if !self.removed {
            self.stats.save();
        }
    }

    /// Keyboard input stops reaching us: end recording and key capture.
    fn on_focus_lost(&mut self) {
        self.stop_recording();
        if let Some(p) = self.view.picker.as_mut() {
            p.capturing = false;
        }
        window::invalidate();
    }

    fn on_act(&mut self, a: Act) {
        match a {
            Act::Nav(p) => {
                self.stop_recording();
                self.view.page = p;
            }
            Act::Activate(i) => self.activate_stage(i),
            Act::Toggle(i) => self.toggle_stage(i),
            Act::Slider(_) => {}
            Act::Polling(i) => self.set_polling(i),
            Act::Period(i) => self.set_period(i),
            Act::Autostart => self.set_autostart(!self.view.autostart),
            Act::Install => self.answer_setup(true, self.view.autostart),
            Act::Uninstall => self.uninstall(),
            Act::SetupAutostart => self.view.setup_autostart = !self.view.setup_autostart,
            Act::SetupInstall => self.answer_setup(true, self.view.setup_autostart),
            Act::SetupSkip => self.answer_setup(false, self.view.setup_autostart),
            Act::Theme(i) => self.set_theme(i),
            Act::Lang(i) => self.set_lang(i),
            Act::Effect(_) | Act::Brightness(_) | Act::Speed(_) | Act::Direction(_) | Act::Steady(_) | Act::Breath(_) => {
                self.edit_lighting(a)
            }
            Act::DarkMoving => {
                self.cfg.power.dark_while_moving = !self.cfg.power.dark_while_moving;
                self.push_power();
            }
            Act::Sleep(i) => {
                self.cfg.power.sleep_min = store::SLEEP_CHOICES[i.min(store::SLEEP_CHOICES.len() - 1)];
                self.push_power();
            }
            Act::DpiColor(i) => self.cycle_dpi_color(i),
            Act::Debounce(i) => {
                self.cfg.debounce = store::DEBOUNCE_CHOICES[i.min(store::DEBOUNCE_CHOICES.len() - 1)];
                self.view.debounce = self.cfg.debounce;
                self.mouse.send(Command::SetDebounce(self.cfg.debounce));
                store::save_config(&self.cfg);
            }
            Act::EditButton(i) => {
                self.view.notice.clear();
                self.view.picker = Some(Picker::for_button(i, self.cfg.buttons[i]));
            }
            Act::ResetButtons => self.set_buttons(DEFAULT_ACTIONS),
            Act::PickTab(_) | Act::PickMod(_) | Act::PickCapture | Act::PickInterval(_) | Act::PickLoops(_) => self.edit_picker(a),
            Act::PickMacro(_) | Act::PickMode(_) => self.edit_picker(a),
            Act::PickAction(action) => self.assign(action),
            Act::PickApply => {
                if let Some(action) = self.view.picker.and_then(|p| p.built()) {
                    self.assign(action);
                }
            }
            Act::PickCancel => self.view.picker = None,
            Act::MacroSel(i) => {
                self.stop_recording();
                self.view.macro_sel = i.min(self.cfg.macros.len() - 1);
            }
            Act::MacroRecord => {
                if self.view.recording {
                    self.stop_recording();
                } else {
                    self.cfg.macros[self.view.macro_sel].clear();
                    self.view.macros = self.cfg.macros.clone();
                    self.view.recording = true;
                    self.last_step = None;
                }
            }
            Act::MacroClear => {
                self.view.recording = false;
                self.cfg.macros[self.view.macro_sel].clear();
                self.macros_changed();
            }
            Act::MacroDelay(d) => {
                self.view.delay_mode = d;
                // A fixed delay also rewrites the steps already recorded.
                if let DelayMode::Fixed(ms) = d {
                    self.cfg.macros[self.view.macro_sel].iter_mut().for_each(|e| e.delay_ms = ms);
                    self.macros_changed();
                }
            }
            Act::ChangeMouse => self.open_detect(hid_link::list_mice()),
            Act::BasicDpi => {}
            Act::BasicRate(hz) => {
                self.view.basic.rate = Some(hz);
                self.mouse.send(Command::SetBasicRate(hz));
            }
            Act::DetectSel(i) => {
                if let Some(d) = self.view.detect.as_mut() {
                    d.sel = i.min(d.rows.len().saturating_sub(1));
                }
            }
            Act::DetectManual => {
                if let Some(d) = self.view.detect.as_mut() {
                    (d.rows, d.sel, d.manual) = (model_rows(), 0, true);
                }
            }
            Act::DetectUse => self.choose_device(),
            Act::Lod(lod) => {
                self.cfg.lod = lod;
                self.view.lod = lod;
                self.mouse.send(Command::SetLod(lod));
                store::save_config(&self.cfg);
            }
        }
        window::invalidate();
    }

    // ---- device status ----

    fn on_mouse(&mut self, ev: Event) {
        if !belongs_to(&ev, &self.target) {
            return;
        }
        let (link, level) = match ev {
            Event::Device(info) => {
                self.device_info = Some(info);
                self.refresh_device();
                return;
            }
            Event::Ident(ident, ability) => {
                self.on_ident(ident, ability);
                return;
            }
            Event::Basic(r) => {
                self.basic_online = true;
                self.view.basic.rate = r.rate_hz;
                self.view.basic.dpi = r.dpi;
                if let Some(s) = r.stage.filter(|s| Some(*s) != self.cfg.last_stage) {
                    self.cfg.last_stage = Some(s);
                    store::save_config(&self.cfg);
                }
                self.view.basic.stage = r.stage.or(self.cfg.last_stage);
                // Stage-only mice: DPI from the catalogue table.
                if r.dpi.is_none() {
                    self.view.basic.dpi = self.stage_dpi();
                }
                let dpi = self.view.basic.dpi;
                self.view.has_battery = r.battery.is_some();
                self.view.battery = r.battery.unwrap_or(0);
                self.view.charge = if r.charging { Charge::Charging } else { Charge::Discharging };
                if let Some(d) = dpi {
                    self.stats.set_dpi(d);
                }
                let level = match r.battery {
                    Some(pct) => {
                        self.stats.record_battery(pct, r.charging);
                        Level::Battery { pct, charging: r.charging }
                    }
                    None => Level::Offline,
                };
                (T::LinkConnected, level)
            }
            Event::Status(s) => {
                self.apply_status(s);
                let charging = s.charge == protocol::Charge::Charging;
                (T::LinkConnected, Level::Battery { pct: s.battery, charging })
            }
            Event::Offline => {
                self.status = None;
                self.basic_online = false;
                (T::LinkAsleep, Level::Offline)
            }
            Event::Absent => {
                self.status = None;
                self.basic_online = false;
                (T::LinkNoReceiver, Level::Offline)
            }
        };

        self.view.connected = self.status.is_some() || self.basic_online;
        self.link = link;
        self.view.link = tr(link).into();
        self.level = level;
        self.refresh_device();
        self.update_tray();
        window::invalidate();
    }

    fn update_tray(&self) {
        let tip = match self.level {
            Level::Battery { pct, charging } => {
                let base = format!("{} · {}", crate::APP_NAME, fill(tr(T::TipBattery), &[&pct.to_string()]));
                if charging { format!("{base} · {}", tr(T::TipCharging)) } else { base }
            }
            Level::Offline => format!("{} · {}", crate::APP_NAME, tr(self.link)),
        };
        window::set_tray(self.level, &tip);
    }

    fn apply_status(&mut self, s: Status) {
        self.status = Some(s);
        // The sensor decides the DPI table (steps and values).
        if protocol::sensor().id != s.sensor {
            protocol::set_sensor(s.sensor);
            self.view.steps = protocol::dpi_steps();
            self.sync_stages();
        }
        self.view.battery = s.battery;
        self.view.charge = match s.charge {
            protocol::Charge::Discharging => Charge::Discharging,
            protocol::Charge::Charging => Charge::Charging,
            protocol::Charge::Full => Charge::Full,
        };

        // The DPI button on the mouse changes the stage behind our back.
        if s.active_stage != self.cfg.dpi.active {
            self.cfg.dpi.active = s.active_stage;
            if !self.removed {
                store::save_config(&self.cfg);
            }
            self.sync_stages();
        }

        let dpi = protocol::dpi_of(self.cfg.dpi.stages[s.active_stage]);
        self.stats.set_dpi(dpi);
        self.stats.record_battery(s.battery, s.charge == protocol::Charge::Charging);
    }

    // ---- DPI ----

    fn sync_stages(&mut self) {
        let d = &self.cfg.dpi;
        self.view.stages = (0..STAGE_COUNT)
            .map(|i| StageView { dpi: protocol::dpi_of(d.stages[i]), index: d.stages[i], enabled: d.enabled & (1 << i) != 0 })
            .collect();
        self.view.active = d.active;
    }

    fn push_dpi(&mut self) {
        self.sync_stages();
        self.mouse.send(Command::SetDpi(self.cfg.dpi.clone()));
        store::save_config(&self.cfg);
    }

    /// Single-DPI slider: preview while dragging, send on release.
    fn drag_basic_dpi(&mut self, index: usize, phase: Phase) {
        let Some(&dpi) = self.view.basic.values.get(index) else {
            return;
        };
        self.view.basic.dpi = Some(dpi);
        if phase == Phase::Done {
            self.mouse.send(Command::SetBasicDpi(dpi));
        }
        window::invalidate();
    }

    /// Slider drag: show the value, send only on release.
    fn preview_stage(&mut self, i: usize, index: usize) {
        if let Some(s) = self.view.stages.get_mut(i) {
            s.index = index;
            s.dpi = protocol::dpi_of(index);
            window::invalidate();
        }
    }

    fn set_stage(&mut self, i: usize, index: usize) {
        if i >= STAGE_COUNT {
            return;
        }
        self.cfg.dpi.stages[i] = index.min(protocol::dpi_steps() - 1);
        self.push_dpi();
        window::invalidate();
    }

    /// At least one stage stays on, and the active stage can't be switched off.
    fn toggle_stage(&mut self, i: usize) {
        if i >= STAGE_COUNT || i == self.cfg.dpi.active {
            return;
        }
        let next = self.cfg.dpi.enabled ^ (1 << i);
        if next == 0 {
            return;
        }
        self.cfg.dpi.enabled = next;
        self.push_dpi();
    }

    fn activate_stage(&mut self, i: usize) {
        if i >= STAGE_COUNT || self.cfg.dpi.enabled & (1 << i) == 0 {
            return;
        }
        self.cfg.dpi.active = i;
        self.push_dpi();
    }

    // ---- device ----

    /// DPI of the current stage from the model's table, e.g. stage 2 -> 2400.
    fn stage_dpi(&self) -> Option<u32> {
        let model = self.cfg.device.as_deref().and_then(devices::by_id)?;
        let stage = self.view.basic.stage? as usize;
        model.stage_dpi.get(stage.checked_sub(1)?).copied()
    }

    /// The chosen mouse's entry among the detected ones (no-driver models
    /// and "other mouse" only; driver-based mice report their own identity).
    fn chosen_detected(&self) -> Option<&DeviceInfo> {
        match self.cfg.device.as_deref().map(devices::by_id) {
            Some(Some(m)) if m.family != Family::NoDriver => None,
            Some(Some(m)) => self.mice.iter().find(|d| devices::match_device(d) == Match::Known(m)),
            _ => self.mice.iter().find(|d| devices::match_device(d) == Match::Unknown).or(self.mice.first()),
        }
    }

    fn retarget(&mut self, t: Target) {
        self.target = t.clone();
        self.mouse.send(Command::Target(t));
    }

    /// Mice without a driver but connected over Bluetooth still report a
    /// battery through Windows: point the mouse service at their address.
    fn retarget_bluetooth(&mut self) {
        let Some(addr) = self.chosen_detected().and_then(|d| d.bt_address.clone()) else {
            return;
        };
        self.retarget(Target::Bluetooth(addr));
    }

    /// A driver-based mouse opened: name, DPI range and report rates.
    fn on_ident(&mut self, ident: Ident, ability: Ability) {
        let model = self.cfg.device.as_deref().and_then(devices::by_id);
        if let (Some(m), false) = (model, ident.name.is_empty()) {
            if m.pids.is_empty() {
                self.view.device_name = format!("{} {}", m.vendor, ident.name.trim_start_matches(m.vendor).trim());
            }
        }
        self.view.basic.values = ability.dpi.map(|r| r.values()).unwrap_or_default();
        self.view.basic.rates = ability.rates;
        self.view.basic.stages = ability.stages;
        self.view.has_battery = ability.battery;
        self.ident = Some(ident);
        self.refresh_device();
    }

    fn on_mice(&mut self, mice: Vec<DeviceInfo>) {
        self.mice = mice.clone();
        self.retarget_bluetooth();
        self.refresh_device();
        if self.cfg.device.is_none() {
            self.open_detect(mice);
        }
    }

    fn open_detect(&mut self, mice: Vec<DeviceInfo>) {
        self.mice = mice.clone();
        let empty = mice.is_empty();
        let rows = if empty { model_rows() } else { detect_rows(&mice) };
        // Preselect the first mouse we can drive.
        let sel = rows.iter().position(|r| r.choice != store::GENERIC_DEVICE).unwrap_or(0);
        self.view.detect = Some(Detect { rows, sel, manual: empty, empty });
        window::invalidate();
    }

    fn choose_device(&mut self) {
        let Some(d) = self.view.detect.take() else {
            return;
        };
        let Some(row) = d.rows.get(d.sel) else {
            return;
        };
        self.cfg.device = Some(row.choice.clone());
        store::save_config(&self.cfg);

        // New driver: forget what the previous one reported.
        self.retarget(target_of(self.cfg.device.as_deref()));
        (self.status, self.ident, self.device_info, self.basic_online) = (None, None, None, false);
        let caps = caps_of(self.cfg.device.as_deref());
        self.view.caps = caps;
        forget_mouse(&mut self.view, caps);
        self.retarget_bluetooth();

        self.view.device_name = device_name(self.cfg.device.as_deref());
        self.stats.set_vendor(stats_vendor(self.cfg.device.as_deref()));
        if !view::page_visible(self.view.page, &self.view.caps) {
            self.view.page = 0;
        }
        self.refresh_device();
    }

    /// Rows of the Device page from the catalogue, USB identity and status.
    fn refresh_device(&mut self) {
        let model = self.cfg.device.as_deref().and_then(devices::by_id);
        // With a driver: the opened interface; otherwise the detected entry.
        let owned = match model {
            Some(m) if m.family != Family::NoDriver => self.device_info.clone(),
            _ => self.chosen_detected().cloned(),
        };
        let info = owned.as_ref();
        let mut rows: Vec<(T, String)> = Vec::new();

        // "Other mouse": count only that mouse, not every mouse connected.
        if model.is_none() {
            self.stats.set_vendor(info.map(|i| i.vid));
        }

        // "Other mouse": show the name the detected mouse reports.
        if model.is_none() {
            let product = info.map(|i| i.product.clone()).filter(|p| !p.is_empty());
            self.view.device_name = product.unwrap_or_else(|| tr(T::GenericMouse).into());
        }
        rows.push((T::DevModel, self.view.device_name.clone()));
        if let Some(m) = model {
            rows.push((T::DevSupport, support_badge(m.support).0.into()));
            let link = info.and_then(|i| m.link_of(i.pid)).map(|l| match l {
                Link::Receiver => tr(T::LinkReceiver).to_string(),
                Link::Cable => tr(T::LinkCable).to_string(),
            });
            rows.push((T::DevConnection, link.unwrap_or_default()));
        }
        let ident = self.ident.as_ref().filter(|_| model.is_some_and(|m| m.family.has_driver()));
        if let Some(addr) = info.and_then(|i| i.bt_address.as_deref()) {
            // AA:BB:CC:DD:EE:FF
            let pretty: Vec<&str> = (0..addr.len()).step_by(2).map(|k| &addr[k..k + 2]).collect();
            rows.push((T::DevConnection, format!("Bluetooth LE · {}", pretty.join(":"))));
        }
        if let Some(i) = info {
            rows.push((T::DevUsbId, format!("{:04X}:{:04X}", i.vid, i.pid)));
            rows.push((T::DevManufacturer, i.manufacturer.clone()));
            rows.push((T::DevProduct, i.product.clone()));
            let fw = ident.map(|d| d.firmware.clone()).filter(|f| !f.is_empty()).unwrap_or_else(|| devices::bcd_version(i.version));
            rows.push((T::DevFirmware, fw));
            let serial = ident.map(|d| d.serial.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| i.serial.clone());
            rows.push((T::DevSerial, serial));
        }
        if let Some(d) = ident {
            rows.push((T::DevProtocol, d.protocol.clone()));
        }
        if let Some(s) = self.status.filter(|_| model.is_some()) {
            let sensor = devices::sensor_name(s.sensor).map_or_else(|| format!("0x{:02X}", s.sensor), str::to_string);
            rows.push((T::DevSensor, sensor));
            rows.push((T::DevProtocol, "Telink · M022".into()));
            // Payload bytes of the status reply, for bug reports.
            let raw: Vec<String> = s.raw[5..24].iter().map(|b| format!("{b:02X}")).collect();
            rows.push((T::DevStatus, raw.join(" ")));
        }

        // Only driver-based or Bluetooth mice have a connection state to report.
        let driven = model.is_some_and(|m| m.family != Family::NoDriver) || info.is_some_and(|i| i.bt_address.is_some());
        let offline = driven && !self.view.connected && info.is_some();
        let experimental = model.is_some_and(|m| m.support == Support::Experimental);
        self.view.device = DeviceView { rows, offline, experimental };
        window::invalidate();
    }

    // ---- buttons ----

    /// Applies a new layout; refused if no button would stay a left click.
    fn set_buttons(&mut self, buttons: [Action; protocol::BUTTON_COUNT]) {
        if !buttons.contains(&Action::Mouse(MB_LEFT)) {
            self.view.notice = tr(T::NeedLeft).into();
            return;
        }
        self.view.notice.clear();
        self.cfg.buttons = buttons;
        self.view.buttons = buttons;
        self.mouse.send(Command::SetButtons(self.cfg.buttons, self.cfg.macros.clone()));
        store::save_config(&self.cfg);
    }

    fn assign(&mut self, action: Action) {
        let Some(p) = self.view.picker.take() else {
            return;
        };
        let mut b = self.cfg.buttons;
        b[p.button] = action;
        self.set_buttons(b);
    }

    fn edit_picker(&mut self, a: Act) {
        let Some(p) = self.view.picker.as_mut() else {
            return;
        };
        match a {
            Act::PickTab(t) => {
                p.tab = t;
                p.capturing = false;
            }
            Act::PickMod(bit) => p.mods ^= bit,
            Act::PickCapture => {
                p.capturing = true;
                p.held = 0;
            }
            Act::PickInterval(ms) => p.interval = ms,
            Act::PickLoops(n) => p.loops = n,
            Act::PickMacro(i) => p.macro_index = i,
            Act::PickMode(m) => p.mode = m,
            _ => {}
        }
    }

    /// Keyboard: shortcut capture in the picker, macro recording, Esc to close.
    fn on_key(&mut self, k: KeyEvent) -> bool {
        const VK_ESCAPE: u16 = 0x1B;

        if let Some(p) = self.view.picker.as_mut() {
            if !p.capturing {
                if k.down && k.vk == VK_ESCAPE {
                    self.view.picker = None;
                    window::invalidate();
                    return true;
                }
                return false;
            }
            // Modifiers alone don't end the capture; the next real key does.
            // Modifiers pressed with it win; with none, the chips stay as set.
            match keys::macro_mod_of(k.vk, k.extended) {
                Some(bits) if k.down => p.held |= shortcut_mod(bits),
                Some(_) => {}
                None if k.down => {
                    if let Some(usage) = keys::hid_of(k.vk) {
                        let m = window::modifiers();
                        let bit = |on: bool, b: u8| if on { b } else { 0 };
                        let live = bit(m.ctrl, MOD_CTRL) | bit(m.shift, MOD_SHIFT) | bit(m.alt, MOD_ALT) | bit(m.win, MOD_WIN);
                        if p.held | live != 0 {
                            p.mods = p.held | live;
                        }
                        p.key = Some(usage);
                        p.capturing = false;
                    }
                }
                None => {}
            }
            window::invalidate();
            return true;
        }

        if !self.view.recording {
            return false;
        }
        const VK_F4: u16 = 0x73;
        if k.vk == VK_F4 && window::modifiers().alt {
            self.stop_recording();
            return false;
        }
        if !k.repeat {
            self.record_key(k);
        }
        true
    }

    fn record_key(&mut self, k: KeyEvent) {
        let edge = if k.down { Edge::Down } else { Edge::Up };
        let ev = match (keys::macro_mod_of(k.vk, k.extended), keys::hid_of(k.vk)) {
            (Some(bit), _) => Ev::Mods(bit, edge),
            (None, Some(usage)) => Ev::Key(usage, edge),
            _ => return,
        };

        let now = Instant::now();
        let gap = match self.view.delay_mode {
            DelayMode::Recorded => self.last_step.map_or(0, |t| now.duration_since(t).as_millis().min(DELAY_MAX_MS as u128) as u16),
            DelayMode::Fixed(ms) => ms,
        };
        self.last_step = Some(now);

        let m = &mut self.cfg.macros[self.view.macro_sel];
        if m.len() >= MACRO_MAX_EVENTS {
            self.stop_recording();
            return;
        }
        // The delay belongs to the step before the gap.
        if let Some(last) = m.last_mut() {
            last.delay_ms = gap;
        }
        m.push(MacroEvent { ev, delay_ms: 0 });
        self.view.macros = self.cfg.macros.clone();
        window::invalidate();
    }

    fn stop_recording(&mut self) {
        if !self.view.recording {
            return;
        }
        self.view.recording = false;
        let sel = self.view.macro_sel;
        if let (DelayMode::Fixed(ms), Some(last)) = (self.view.delay_mode, self.cfg.macros[sel].last_mut()) {
            last.delay_ms = ms;
        }
        // Keys still down (focus lost, Alt+Tab…) get a release so the mouse never holds them.
        self.cfg.macros[sel] = protocol::balance(std::mem::take(&mut self.cfg.macros[sel]));
        self.macros_changed();
    }

    /// Saves macros and re-sends the buttons when a macro is in use.
    fn macros_changed(&mut self) {
        self.view.macros = self.cfg.macros.clone();
        store::save_config(&self.cfg);
        if self.cfg.buttons.iter().any(|a| matches!(a, Action::Macro { .. })) {
            self.mouse.send(Command::SetButtons(self.cfg.buttons, self.cfg.macros.clone()));
        }
    }

    // ---- lighting ----

    fn edit_lighting(&mut self, a: Act) {
        let l = &mut self.cfg.lighting;
        match a {
            Act::Effect(i) => l.effect = Effect::ALL[i.min(Effect::ALL.len() - 1)],
            Act::Brightness(b) => l.brightness = b,
            Act::Speed(sp) => l.speed = sp,
            Act::Direction(d) => l.direction = d,
            Act::Steady(i) => l.steady = view::PALETTE[i.min(view::PALETTE.len() - 1)],
            Act::Breath(i) => {
                // Toggle; keep at least one color and at most the firmware limit.
                let c = view::PALETTE[i.min(view::PALETTE.len() - 1)];
                match l.breathing.iter().position(|&x| x == c) {
                    Some(k) if l.breathing.len() > 1 => {
                        l.breathing.remove(k);
                    }
                    None if l.breathing.len() < BREATHING_MAX => l.breathing.push(c),
                    _ => return,
                }
            }
            _ => return,
        }
        self.view.lighting = self.cfg.lighting.clone();
        self.mouse.send(Command::SetLighting(self.cfg.lighting.clone()));
        store::save_config(&self.cfg);
    }

    fn push_power(&mut self) {
        self.view.power = self.cfg.power;
        self.mouse.send(Command::SetPower(self.cfg.power));
        store::save_config(&self.cfg);
    }

    /// Next palette color for a stage's indicator LED.
    fn cycle_dpi_color(&mut self, i: usize) {
        let Some(&cur) = self.cfg.dpi_colors.get(i) else {
            return;
        };
        let next = view::PALETTE.iter().position(|&c| c == cur).map_or(0, |k| (k + 1) % view::PALETTE.len());
        self.cfg.dpi_colors[i] = view::PALETTE[next];
        self.view.dpi_colors = self.cfg.dpi_colors;
        self.mouse.send(Command::SetDpiColors(self.cfg.dpi_colors));
        store::save_config(&self.cfg);
    }

    // ---- polling ----

    fn set_polling(&mut self, i: usize) {
        let Some(&rate) = PollingRate::ALL.get(i) else {
            return;
        };
        self.cfg.polling = rate;
        self.view.polling = i;
        self.mouse.send(Command::SetPolling(rate));
        store::save_config(&self.cfg);
    }

    // ---- settings / install ----

    fn set_autostart(&mut self, on: bool) {
        autostart::set(if on { Autostart::On } else { Autostart::Off });
        self.view.autostart = autostart::is_enabled();
    }

    /// Install into the user profile, or just run from here.
    fn answer_setup(&mut self, install_now: bool, auto: bool) {
        self.cfg.setup_asked = true;
        store::save_config(&self.cfg);
        self.view.ask_setup = false;

        // Autostart needs the installed copy, so "just run" ignores it.
        if !install_now {
            return;
        }

        let mode = if auto { Autostart::On } else { Autostart::Off };
        match install::install(mode) {
            Ok(()) => {
                self.stats.save();
                window::quit();
            }
            Err(e) => {
                self.view.error = fill(tr(T::InstallFailed), &[&e]);
                self.view.page = view::PAGE_SETTINGS;
            }
        }
    }

    fn set_theme(&mut self, pick: usize) {
        self.cfg.theme = match pick {
            0 => theme::Choice::System,
            i => theme::Choice::Fixed((i - 1).min(THEMES.len() - 1)),
        };
        self.view.theme_pick = pick;
        apply_look(&self.cfg);
        store::save_config(&self.cfg);
    }

    fn set_lang(&mut self, pick: usize) {
        self.cfg.lang = pick.checked_sub(1).and_then(|i| Lang::ALL.get(i).copied());
        self.view.lang_pick = pick;
        apply_look(&self.cfg);
        store::save_config(&self.cfg);

        // Texts built from translations must be rebuilt.
        self.view.link = tr(self.link).into();
        self.update_tray();
        self.refresh_stats();
    }

    fn uninstall(&mut self) {
        if install::uninstall(install::Origin::InApp) {
            self.removed = true;
            window::quit();
        }
    }

    // ---- statistics ----

    fn set_period(&mut self, i: usize) {
        self.view.period = i;
        self.period = match i {
            0 => Period::Today,
            1 => Period::Week,
            _ => Period::All,
        };
        self.refresh_stats();
    }

    fn refresh_stats(&mut self) {
        let today = self.stats.totals(Period::Today);
        self.view.today_clicks = group(today.clicks());
        self.view.today_distance = distance(today.distance_m);
        self.view.measured = self.stats.measured_hz().map(|hz| format!("{hz} Hz")).unwrap_or_else(|| "—".into());

        let t = self.stats.totals(self.period);
        self.view.tiles = vec![
            (tr(T::TileClicks), group(t.clicks())),
            (tr(T::TileLeft), group(t.left)),
            (tr(T::TileRight), group(t.right)),
            (tr(T::TileMiddleSide), group(t.middle + t.side)),
            (tr(T::TileWheel), group(t.wheel)),
            (tr(T::TileDistance), distance(t.distance_m)),
        ];

        let history = self.stats.battery_history();
        self.view.chart = chart(&history, stats::unix_now());
        self.view.battery_note = battery_note(stats::hours_left(&history));
        self.view.chart_caption = if history.is_empty() { String::new() } else { fill(tr(T::ChartSamples), &[&history.len().to_string()]) };
        window::invalidate();
    }
}

// ---- formatting ----------------------------------------------------------

/// `12345` -> `"12 345"` (non-breaking space).
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('\u{a0}');
        }
        out.push(ch);
    }
    out
}

fn distance(m: f64) -> String {
    if m < 1000.0 {
        return format!("{m:.0} m");
    }
    format!("{:.2} km", m / 1000.0).replace('.', ",")
}

fn battery_note(hours: Option<f64>) -> String {
    match hours {
        None => tr(T::EtaPending).into(),
        Some(h) if h >= 48.0 => fill(tr(T::EtaDays), &[&(h / 24.0).round().to_string()]),
        Some(h) => fill(tr(T::EtaHours), &[&h.round().max(1.0).to_string()]),
    }
}

/// Battery samples of the last days as (0..1 time, %) points.
fn chart(samples: &[stats::BatterySample], now: u64) -> Vec<(f32, f32)> {
    let span = CHART_DAYS * 24 * 3600;
    let start = now.saturating_sub(span);
    samples
        .iter()
        .filter(|s| s.t >= start)
        .map(|s| ((s.t - start) as f32 / span as f32, s.pct as f32))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_to_driverless_mouse_drops_old_battery() {
        // Lionel was connected at 56 %, then "other mouse" is chosen.
        let mut v = initial_view(&AppConfig::default(), false);
        (v.connected, v.has_battery, v.battery) = (true, true, 56);
        forget_mouse(&mut v, CAPS_NONE);
        assert!(!v.connected);
        assert!(!v.has_battery);
        assert_eq!(v.battery, 0);
    }

    #[test]
    fn late_events_of_previous_driver_are_ignored() {
        let lionel_status = Event::Offline;
        assert!(!belongs_to(&lionel_status, &Target::Nothing));
        let basic = Event::Basic(crate::drivers::Reading::default());
        assert!(!belongs_to(&basic, &Target::Lionel));
        assert!(belongs_to(&basic, &Target::Bluetooth("B66B18D71E00".into())));
        assert!(belongs_to(&Event::Offline, &Target::Lionel));
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(group(0), "0");
        assert_eq!(group(1234567), "1\u{a0}234\u{a0}567");
    }

    #[test]
    fn formats_distance() {
        assert_eq!(distance(85.4), "85 m");
        assert_eq!(distance(1530.0), "1,53 km");
    }
}
