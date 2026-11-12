//! Controller: the only place that connects the UI with the services.
//!
//! ```text
//!   platform window --Act--> Controller --> MouseService --> HidLink
//!        ^                       |-------> Stats (input hook)
//!        +-- view::draw(View) ---+-------> store / autostart / install
//! ```
//!
//! Lives on the UI thread in a thread-local; other threads reach it with
//! `window::post(.. with_ctrl(..))`.

mod profiles;
mod report;
mod updater;
mod verify;
mod watch;

use crate::feedback;

use crate::devices::{self, CAPS_NONE, Caps, DpiUi, Entry, Family, Link, Match, Model, Support};
use crate::drivers::{Ability, Ident, Pick};
use crate::mouse::Target;
use crate::hid_link::{self, DeviceInfo, HidDevice};
use crate::gfx::Gfx;
use crate::i18n::{self, Lang, T, fill, tr};
use crate::mouse::{Command, Event, MouseService, Probe};
use crate::keys;
use crate::protocol::{
    self, Action, BREATHING_MAX, DEFAULT_ACTIONS, DELAY_MAX_MS, Edge, Effect, Ev, MACRO_MAX_EVENTS, MB_LEFT, MOD_ALT,
    MOD_CTRL, MOD_SHIFT, MOD_WIN, MacroEvent, PollingRate, STAGE_COUNT, Status,
};
use crate::platform::{self, Access, Autostart, Shade, TitleColors, autostart, hid, install, window};
use crate::shell::{Hooks, KeyEvent, Mods, Phase};
use crate::stats::{self, Period, Stats};
use crate::theme::{self, THEMES};
use crate::store::{self, AppConfig, KnownMouse};
use crate::tray::{Level, TrayAction};
use crate::view::{
    self, Act, Answer, Ask, Badge, Banner, BasicView, BrandRow, Charge, CheckView, DelayMode, Detect, DetectRow, DetectStep, DeviceView, Dialog,
    Focus, Found, Hit, Page, Pane, PickRow, Picker, ReportKind, ReportStep, ReportView, Sending, StageView, Sync, Toast, UpdateView, View, Web,
};
use updater::{Trigger, Updates};
use verify::{Check, Knobs, Start, Verify};
use std::cell::RefCell;
use std::time::{Duration, Instant};

const TIMER_REFRESH: usize = 1;
const TIMER_SAVE: usize = 2;
const TIMER_TOAST: usize = 3;
const TIMER_FADE: usize = 4;
const TIMER_PULSE: usize = 5;
const TIMER_SCAN: usize = 6;
const TIMER_UPDATE: usize = 7;
/// Subtle animations: only while the window is shown.
const TIMER_FX: usize = 8;
/// Five frames a second are plenty for a mark that breathes in 2.4 s.
const FX_TICK_MS: u32 = 200;
const STATUS_PULSE: Duration = Duration::from_millis(2400);
/// How often connected mice are listed to notice a new one.
const SCAN_MS: u32 = 4000;
/// Longest wait at exit for the mouse to take back changed settings.
const FLUSH_LIMIT: Duration = Duration::from_secs(2);
const REFRESH_MS: u32 = 1000;
const SAVE_MS: u32 = 60_000;
/// How long a toast (and its Undo) stays.
const TOAST_MS: u32 = 5000;
const FADE_TICK_MS: u32 = 50;
const PULSE_TICK_MS: u32 = 50;
/// One breath of the recording dot.
const PULSE_PERIOD: Duration = Duration::from_secs(1);
/// "Saved to mouse" stays solid, then fades out.
const SAVED_HOLD: Duration = Duration::from_millis(1500);
const SAVED_FADE: Duration = Duration::from_millis(1000);
const DAY_SECS: u64 = 24 * 3600;
const CHART_DAYS: u64 = 7;
/// Most day labels under the battery chart.
const CHART_MAX_TICKS: u64 = 7;
const REPO_URL: &str = "https://github.com/Emlynix/whisker";
const SITE_URL: &str = "https://whisker.emlynix.com";
const NEW_ISSUE_URL: &str = "https://github.com/Emlynix/whisker/issues/new";

/// Windows virtual-key codes the controller reacts to (Linux maps to them).
const VK_TAB: u16 = 0x09;
const VK_RETURN: u16 = 0x0D;
const VK_ESCAPE: u16 = 0x1B;
const VK_SPACE: u16 = 0x20;
const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;
const VK_PRIOR: u16 = 0x21;
const VK_NEXT: u16 = 0x22;
const VK_1: u16 = 0x31;
const VK_Z: u16 = 0x5A;
const VK_COMMA: u16 = 0xBC;

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
    /// Mice found by the last scan or detection. The detect dialog opens
    /// with these and a scan puts the fresh list in.
    mice: Vec<DeviceInfo>,
    /// Identity reported by a driver-based mouse.
    ident: Option<Ident>,
    /// A driver-based mouse answered the last poll.
    basic_online: bool,
    /// Driver the mouse service was last pointed at.
    target: Target,
    /// What the toast's Undo (and Ctrl+Z) puts back.
    undo: Option<Undo>,
    /// When the mouse confirmed the last change ("Saved to mouse" fade).
    saved_at: Option<Instant>,
    /// Clickable acts of the last frame in order, for Tab focus.
    acts: Vec<Act>,
    /// Start of the macro recording, for the pulsing dot.
    rec_start: Instant,
    /// The window is on screen (else new mice are announced by notification).
    visible: bool,
    /// Keys (`mouse_key`) of the mice connected at the last scan; `None`
    /// until the startup list arrived.
    seen: Option<Vec<String>>,
    /// A scan is running on its thread.
    scanning: bool,
    /// The mouse the new-mouse banner offers.
    offer: Option<DeviceInfo>,
    /// "Help add your mouse" while open.
    wizard: Option<Wizard>,
    /// Values the guided checks must see back on the mouse.
    restore: Option<Restore>,
    updates: Updates,
    /// When the app started: the clock of the animations.
    started: Instant,
    /// Number of the last report sent; a send in flight carries it.
    send_seq: u64,
    /// One report is on its way: no second one until it is answered.
    send_busy: bool,
    /// A file dialog of the profiles is open: no second one over it.
    asking_file: bool,
}

/// DPI and rate to put back after the checks, re-sent while a reading
/// still differs (a command is lost while the driver reopens).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Restore {
    dpi: Option<u32>,
    rate: Option<u32>,
    /// Readings left to wait for the old values.
    tries: u8,
}

/// Readings (about 2 s apart) a restore is re-sent for.
const RESTORE_TRIES: u8 = 10;

/// Commands still needed to put `r` back, given a reading.
fn restore_cmds(r: &Restore, dpi: Option<u32>, rate: Option<u32>) -> Vec<Command> {
    let mut out = Vec::new();
    if let (Some(want), Some(now)) = (r.dpi, dpi) {
        if want != now {
            out.push(Command::SetBasicDpi(want));
        }
    }
    if let (Some(want), Some(now)) = (r.rate, rate) {
        if want != now {
            out.push(Command::SetBasicRate(want));
        }
    }
    out
}

/// State of the report wizard behind `View::report`.
#[derive(Default)]
struct Wizard {
    devices: Vec<HidDevice>,
    probes: Vec<Probe>,
    /// Guided checks while they run.
    verify: Option<Verify>,
    /// Result lines of finished checks.
    checks: Vec<String>,
    /// Number of the send started from this dialog, to know its answer.
    send: Option<u64>,
    /// The report as copied or sent.
    text: String,
}

/// Checks change settings through the mouse service, like the UI does.
impl Knobs for MouseService {
    fn set_dpi(&mut self, dpi: u32) {
        self.send(Command::SetBasicDpi(dpi));
    }

    fn set_rate(&mut self, hz: u32) {
        self.send(Command::SetBasicRate(hz));
    }
}

/// State before a destructive action, kept while its toast shows.
#[derive(Clone, Debug, PartialEq)]
enum Undo {
    Buttons([Action; protocol::BUTTON_COUNT]),
    Macro(usize, Vec<MacroEvent>),
    /// A deleted profile and where it was.
    Profile(usize, store::Profile),
}

/// What must be re-sent to the mouse after an undo.
#[derive(Debug, PartialEq, Eq)]
enum Resend {
    Buttons,
    Macros,
    /// Another profile is the working one.
    Layout,
}

#[derive(Debug, PartialEq, Eq)]
enum Layout {
    Stored,
    Refused,
}

/// Direction of Tab focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Next,
    Prev,
}

pub fn run(mode: StartMode, last: crate::log::Last) {
    let hooks = Hooks {
        draw: hook_draw,
        act: hook_act,
        drag: hook_drag,
        wheel: hook_wheel,
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

    let mut cfg = store::load_config();
    bind_profiles(&mut cfg);
    apply_look(&cfg);
    let first_run = !cfg.setup_asked && !install::is_installed_copy();
    let target = target_of(cfg.device.as_deref());
    let pick = pick_for(cfg.device.as_deref(), cfg.device_usb);
    let ctrl = Controller {
        view: initial_view(&cfg, first_run),
        cfg,
        stats: Stats::start(),
        mouse: MouseService::start(target.clone(), pick, |ev| post(move |c| c.on_mouse(ev))),
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
        undo: None,
        saved_at: None,
        acts: Vec::new(),
        rec_start: Instant::now(),
        visible: false,
        seen: None,
        scanning: false,
        offer: None,
        wizard: None,
        restore: None,
        updates: Updates::default(),
        started: Instant::now(),
        send_seq: 0,
        send_busy: false,
        asking_file: false,
    };
    ctrl.stats.set_vendor(stats_vendor(ctrl.cfg.device.as_deref()));
    CTRL.with(|c| *c.borrow_mut() = Some(ctrl));
    with_ctrl(|c| {
        c.sync_stages();
        c.refresh_stats();
        c.sync_profiles();
    });

    // Which mice are connected; asks the user on first run.
    std::thread::spawn(|| {
        let mice = hid_link::list_mice();
        post(move |c| c.on_mice(mice));
    });

    if !platform::demo() {
        platform::single::listen(|| post(|c| c.show()), || window::post(window::quit));
    }
    window::set_timer(TIMER_SAVE, SAVE_MS);
    window::set_timer(TIMER_SCAN, SCAN_MS);
    window::set_timer(TIMER_UPDATE, updater::POLL_MS);
    with_ctrl(|c| c.auto_check());
    watch::start();
    // Shown with the window: a start into the tray stays quiet.
    if last == crate::log::Last::Crashed {
        with_ctrl(|c| c.view.dialog = Some(Dialog::Crashed));
    }

    if mode == StartMode::Visible || first_run {
        with_ctrl(|c| c.show());
    }
    window::run();
    watch::stop();

    // Guided checks may have changed a setting: put it back before exit.
    with_ctrl(|c| {
        c.close_report();
        c.mouse.flush(FLUSH_LIMIT);
    });
    with_ctrl(|c| {
        if !c.removed {
            c.stats.save();
        }
    });
}

fn initial_view(cfg: &AppConfig, ask_setup: bool) -> View {
    View {
        page: Page::Overview,
        period: 0,
        connected: false,
        link: tr(T::LinkSearching).into(),
        battery: 0,
        battery_approx: false,
        charge: Charge::Discharging,
        battery_note: String::new(),
        stages: Vec::new(),
        active: cfg.dpi.active,
        steps: protocol::dpi_steps(),
        polling: rate_index(cfg.polling),
        measured: None,
        last_rate: None,
        today_clicks: "0".into(),
        today_distance: "0 m".into(),
        tiles: Vec::new(),
        chart: Vec::new(),
        chart_ticks: Vec::new(),
        chart_since: None,
        autostart: autostart::is_enabled(),
        installed: install::is_installed_copy(),
        install_dir: install::install_dir().map(|d| d.display().to_string()).unwrap_or_default(),
        error: String::new(),
        data_dir: store::data_dir().map(|d| d.display().to_string()).unwrap_or_default(),
        ask_setup,
        setup_autostart: true,
        tui: cfg.tui,
        fx_texture: cfg.fx_texture,
        fx_motion: cfg.fx_motion,
        status_pulse: u8::MAX,
        lang_menu: false,
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
        profiles: Vec::new(),
        profile: cfg.profile,
        rename: None,
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
        focus: None,
        toast: None,
        sync: Sync::Hidden,
        dialog: None,
        diagnostics: false,
        pulse: u8::MAX,
        banner: None,
        access: Access::Granted,
        report: None,
        update: UpdateView::new(cfg.update_check, updater::install_kind()),
    }
}

/// Makes the chosen mouse's active profile the working one. Only a model
/// with button settings has profiles; for any other the layout stays.
fn bind_profiles(cfg: &mut AppConfig) {
    let Some(device) = cfg.device.clone().filter(|d| caps_of(Some(d)).buttons) else {
        return;
    };
    cfg.use_device(&device);
}

/// Applies the configured theme and language to the UI.
fn apply_look(cfg: &AppConfig) {
    theme::apply(cfg.theme);
    i18n::set_lang(cfg.lang.unwrap_or_else(i18n::system_lang));
    let t = theme::current();
    let shade = if t.dark { Shade::Dark } else { Shade::Light };
    window::set_title_colors(TitleColors { bg: t.bg, text: t.text, border: t.border, shade });
}

/// Pages to show for the configured mouse. Until one is chosen, show all.
fn caps_of(device: Option<&str>) -> Caps {
    match device {
        None => devices::default_model().caps,
        Some(id) => devices::by_id(id).map_or(CAPS_NONE, |m| m.caps),
    }
}

/// Catalogue caps cut down to what the opened mouse reports: a
/// Logitech M705 has no DPI or report rate to set, so no such pages.
/// Stage mice (Lionel) describe themselves fully in the catalogue.
fn narrow_caps(caps: Caps, ability: &Ability) -> Caps {
    if caps.dpi != DpiUi::Single {
        return caps;
    }
    let dpi = if ability.dpi.is_some() || ability.stages.is_some() { caps.dpi } else { DpiUi::None };
    Caps { dpi, polling: caps.polling && !ability.rates.is_empty(), ..caps }
}

/// Tray tooltip percentage: "50" or "≈ 50".
fn approx_pct(pct: u8, approx: bool) -> String {
    if approx { format!("≈ {pct}") } else { pct.to_string() }
}

/// Devices the driver may open for the configured mouse; any for "other".
fn pick_for(device: Option<&str>, usb: Option<(u16, u16)>) -> Pick {
    device.and_then(devices::by_id).map(|m| m.pick(usb)).unwrap_or_default()
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
    let driver = matches!(t, Target::Hidpp | Target::Razer | Target::Eternico | Target::Sinowealth | Target::SteelSeries | Target::OemHid | Target::Asus | Target::Corsair | Target::Roccat);
    match ev {
        Event::Status(_) => *t == Target::Lionel,
        Event::Device(_) => *t == Target::Lionel || driver,
        Event::Ident(..) => driver,
        Event::Basic(_) => driver || matches!(t, Target::Bluetooth(_)),
        Event::Offline | Event::Absent => *t != Target::Nothing,
        // Asked for by the report wizard, whatever the driver.
        Event::Probe(..) => true,
    }
}

/// Clears what the previous mouse reported; flags follow the new one's
/// capabilities until it reports itself.
fn forget_mouse(v: &mut View, caps: Caps) {
    v.basic = BasicView::default();
    v.connected = false;
    v.access = Access::Granted;
    v.battery = 0;
    v.last_rate = None;
    v.battery_approx = false;
    v.charge = Charge::Discharging;
    v.has_battery = caps.battery;
    v.link = tr(T::LinkSearching).into();
}

fn support_badge(s: Support) -> (&'static str, Badge) {
    match s {
        Support::Full => (tr(T::BadgeFull), Badge::Good),
        Support::Experimental => (tr(T::BadgeExperimental), Badge::Warn),
        Support::Partial => (tr(T::BadgePartial), Badge::Partial),
        Support::StatsOnly => (tr(T::LegendStats), Badge::Muted),
    }
}

/// Statistics count the configured mouse only; "other mouse" counts all mice.
fn stats_vendor(device: Option<&str>) -> Option<u16> {
    match device {
        Some(id) => devices::by_id(id).map(|m| m.vid),
        None => Some(devices::default_model().vid),
    }
}

fn device_name(device: Option<&str>) -> String {
    match device.and_then(devices::by_id) {
        Some(m) => m.full_name(),
        None if device.is_some() => tr(T::GenericMouse).into(),
        None => devices::default_model().full_name(),
    }
}

/// Name of an "other mouse": what it reports, else the generic name.
fn other_name(info: Option<&DeviceInfo>) -> String {
    let product = info.map(|i| i.product.clone()).filter(|p| !p.is_empty());
    product.unwrap_or_else(|| tr(T::GenericMouse).into())
}

/// A connected mouse's catalogue entry and its display name. Unknown mice
/// map to "other mouse" (statistics only).
fn detected(d: &DeviceInfo) -> (String, &'static Model) {
    let unknown = || if d.product.is_empty() { tr(T::UnknownMouse).to_string() } else { d.product.clone() };
    match devices::match_device(d) {
        // Auto entries name the product the vendor reports.
        Match::Known(m) if m.entry == Entry::Auto && !d.product.is_empty() => (format!("{} {}", devices::brand_of(m).title(), d.product), m),
        Match::Known(m) => (m.full_name(), m),
        Match::Guess(m) => (unknown(), m),
        Match::Unknown => (unknown(), other_entry()),
    }
}

/// An auto-detect entry named by what the mouse reports, brand first:
/// "Marathon Mouse/Performance Plus M705" -> "Logitech Marathon …".
fn auto_name(m: &Model, reported: &str) -> String {
    let brand = devices::brand_of(m).title();
    format!("{brand} {}", reported.trim_start_matches(brand.as_str()).trim())
}

/// "Other brand › Other mouse".
fn other_entry() -> &'static Model {
    devices::entry(store::GENERIC_DEVICE).unwrap_or_else(devices::default_model)
}

/// (brand, model) list positions of a catalogue id (old ids too).
fn locate(id: &str) -> Option<(usize, usize)> {
    let id = devices::migrate_id(id);
    devices::brands().iter().enumerate().find_map(|(b, brand)| brand.models.iter().position(|m| m.id == id).map(|k| (b, k)))
}

/// Model rows of brand `b`; the mouse in use (`live`) shows the support
/// its driver found.
fn model_rows(b: usize, live: Option<&(String, Support)>) -> Vec<DetectRow> {
    let Some(brand) = devices::brands().get(b) else {
        return Vec::new();
    };
    brand
        .models
        .iter()
        .map(|m| {
            let proven = live.filter(|(id, _)| *id == m.id).map(|(_, s)| *s);
            let (badge, kind) = support_badge(proven.unwrap_or(m.support));
            let pids = m.pids.iter().map(|(p, _)| format!("{p:04X}")).collect::<Vec<_>>().join("/");
            let sub = if m.vid == 0 { String::new() } else { format!("{:04X}:{}", m.vid, if pids.is_empty() { "*" } else { &pids }) };
            DetectRow { title: m.title(), sub, badge: badge.into(), kind, choice: m.id.clone() }
        })
        .collect()
}

/// The detect dialog for `mice`; preselects the current mouse, else the
/// first detected one Whisker can drive, else the first brand. `reported`
/// is the name the driver read from the mouse in use ("" if none).
fn detect_state(mice: &[DeviceInfo], device: Option<&str>, usb: Option<(u16, u16)>, reported: &str) -> Detect {
    let brands = devices::brands().iter().map(|b| BrandRow { name: b.title(), kind: support_badge(b.best()).1 }).collect();
    let current = device.map(devices::migrate_id);
    let live = current.and_then(devices::by_id).map(|m| (m.id.clone(), devices::support_of(m, reported)));
    let found: Vec<Found> = mice
        .iter()
        .map(|d| {
            let (title, m) = detected(d);
            let (brand, model) = locate(&m.id).unwrap_or((0, 0));
            let usb_here = (d.vid, d.pid);
            let is_current = current == Some(m.id.as_str()) && usb.is_none_or(|u| u == usb_here);
            let (badge, kind) = support_badge(m.support);
            let mut f = Found { title, badge: badge.into(), kind, current: is_current, via: None, brand, model, usb: usb_here };
            if is_current {
                identified(&mut f, m, d, reported);
            }
            f
        })
        .collect();

    let drivable = found.iter().position(|f| f.kind != Badge::Muted);
    let chip = found.iter().position(|f| f.current).or(if current.is_none() { drivable.or((!found.is_empty()).then_some(0)) } else { None });
    let (brand, model) = match (chip, current) {
        (Some(c), _) => (found[c].brand, found[c].model),
        (None, Some(id)) => locate(id).unwrap_or((0, 0)),
        (None, None) => (0, 0),
    };
    // Step 1 lists the detected mice; with none, the catalogue right away.
    let step = if found.is_empty() { DetectStep::List } else { DetectStep::Found };
    let models = model_rows(brand, live.as_ref());
    let mut d = Detect { step, found, brands, models, brand, model, live, chip, kept: None, pane: Pane::Models, brand_top: 0, model_top: 0, keys: false, refetching: false };
    reveal(&mut d);
    d
}

/// The mouse in use as its driver named it, e.g. "Logitech Marathon
/// Mouse/Performance Plus M705" / "Supported · in use · via receiver"
/// instead of "Logitech USB Receiver". Unchanged until identified.
fn identified(f: &mut Found, m: &Model, d: &DeviceInfo, reported: &str) {
    if m.entry != Entry::Auto || reported.is_empty() {
        return;
    }
    let (badge, kind) = support_badge(devices::support_of(m, reported));
    (f.title, f.badge, f.kind) = (auto_name(m, reported), badge.into(), kind);
    f.via = (m.link_of(d.pid) == Some(Link::Receiver)).then_some(T::ViaReceiver);
}

/// Scrolls both lists so their selection is visible.
fn reveal(d: &mut Detect) {
    let keep = |top: &mut usize, sel: usize| {
        if sel < *top {
            *top = sel;
        } else if sel >= *top + view::LIST_ROWS {
            *top = sel + 1 - view::LIST_ROWS;
        }
    };
    keep(&mut d.brand_top, d.brand);
    keep(&mut d.model_top, d.model);
}

/// The dialog while the first list of mice is still being read on its
/// thread: no detected rows and nothing to pick.
fn searching_state(device: Option<&str>) -> Detect {
    let mut d = detect_state(&[], device, None, "");
    d.step = DetectStep::Searching;
    d
}

fn pick_brand(d: &mut Detect, b: usize) {
    if b >= d.brands.len() || b == d.brand {
        return;
    }
    (d.brand, d.models, d.model, d.model_top, d.chip) = (b, model_rows(b, d.live.as_ref()), 0, 0, None);
    reveal(d);
}

fn pick_model(d: &mut Detect, k: usize) {
    if k >= d.models.len() {
        return;
    }
    d.model = k;
    // Still the detected mouse only if the chip points at this model.
    d.chip = d.chip.filter(|&c| (d.found[c].brand, d.found[c].model) == (d.brand, k));
    reveal(d);
}

fn pick_found(d: &mut Detect, i: usize) {
    let Some(f) = d.found.get(i) else {
        return;
    };
    let (b, k) = (f.brand, f.model);
    if b != d.brand {
        (d.brand, d.models, d.model_top) = (b, model_rows(b, d.live.as_ref()), 0);
    }
    (d.model, d.chip) = (k, Some(i));
    reveal(d);
}

/// "My mouse isn't listed": step 2, remembering the step 1 pick.
fn show_list(d: &mut Detect) {
    (d.step, d.kept, d.keys) = (DetectStep::List, d.chip, false);
}

/// "Back" (or Esc) on step 2: step 1 as it was left. False when nothing
/// was detected, so there is no step 1.
fn step_back(d: &mut Detect) -> bool {
    if d.step == DetectStep::Found || d.found.is_empty() {
        return false;
    }
    d.step = DetectStep::Found;
    if let Some(i) = d.kept.filter(|_| d.chip.is_none()) {
        pick_found(d, i);
    }
    true
}

/// Row of the detected mouse `usb` in a rebuilt dialog; none if unplugged.
fn found_at(d: &Detect, usb: (u16, u16)) -> Option<usize> {
    d.found.iter().position(|f| f.usb == usb)
}

/// A scan found other mice while the dialog is open: its rows and
/// preselection replace the old ones (`..` arguments as in `detect_state`),
/// but the step, the pane, the scroll and the mouse picked stay as they
/// were. A dialog that had shown no mouse yet – still looking for mice, or
/// opened with none found – becomes the fresh one.
///
/// ```text
/// old rows  ->  new rows      step 1: pick (by VID:PID) -> same row
///                               step 2: brand, model, tops, pane -> kept
/// ```
fn merge_detect(d: &mut Detect, mice: &[DeviceInfo], device: Option<&str>, usb: Option<(u16, u16)>, reported: &str) {
    let usb_of = |row: Option<usize>| row.and_then(|i| d.found.get(i)).map(|f| f.usb);
    let (pick, keep) = (usb_of(d.chip), usb_of(d.kept));
    let (old_step, pane, brand, model, brand_top, model_top) = (d.step, d.pane, d.brand, d.model, d.brand_top, d.model_top);
    let shown = !d.found.is_empty();
    *d = detect_state(mice, device, usb, reported);
    // Nothing shown yet: the dialog was still looking for mice or opened
    // with none found, so it has nothing of its own to keep.
    if !shown {
        return;
    }
    let fresh = d.chip;
    let (chip, kept) = (pick.and_then(|u| found_at(d, u)), keep.and_then(|u| found_at(d, u)));
    (d.chip, d.kept, d.pane) = (chip, kept, pane);
    if old_step == DetectStep::Found {
        // Step 1: the model list follows the mouse picked; a pick whose
        // mouse is gone falls back to what a dialog opened now preselects.
        if let Some(i) = d.chip.or(fresh) {
            pick_found(d, i);
        }
        return;
    }
    // Step 2: their browsing of the catalogue, its lists still scrolled to
    // the same rows, over the new detected mice.
    (d.step, d.brand) = (DetectStep::List, brand.min(d.brands.len().saturating_sub(1)));
    d.models = model_rows(d.brand, d.live.as_ref());
    d.model = model.min(d.models.len().saturating_sub(1));
    (d.brand_top, d.model_top) = (brand_top.min(d.brands.len().saturating_sub(1)), model_top.min(d.models.len().saturating_sub(1)));
    reveal(d);
}

/// Arrow keys in the dialog; nothing to move while the mice are still being
/// listed. Step 1: Up/Down move among the detected mice. Step 2: Up/Down move
/// in the list, Left/Right (and Tab) switch lists.
fn detect_key(d: &mut Detect, vk: u16) -> bool {
    if d.step == DetectStep::Searching {
        return false;
    }
    let step = |i: usize, n: usize, down: bool| if down { (i + 1).min(n.saturating_sub(1)) } else { i.saturating_sub(1) };
    if d.step == DetectStep::Found {
        if !matches!(vk, VK_UP | VK_DOWN) {
            return false;
        }
        // Nothing picked yet: either key picks the first.
        let i = d.chip.map_or(0, |c| step(c, d.found.len(), vk == VK_DOWN));
        pick_found(d, i);
        return true;
    }
    d.keys = true;
    match (vk, d.pane) {
        (VK_UP | VK_DOWN, Pane::Brands) => pick_brand(d, step(d.brand, d.brands.len(), vk == VK_DOWN)),
        (VK_UP | VK_DOWN, Pane::Models) => pick_model(d, step(d.model, d.models.len(), vk == VK_DOWN)),
        (VK_LEFT, _) => d.pane = Pane::Brands,
        (VK_RIGHT, _) => d.pane = Pane::Models,
        (VK_TAB, Pane::Brands) => d.pane = Pane::Models,
        (VK_TAB, Pane::Models) => d.pane = Pane::Brands,
        _ => return false,
    }
    true
}

/// Mouse wheel over a list: scrolls it (`notches` > 0 = up).
fn detect_wheel(d: &mut Detect, pane: Pane, notches: i32) {
    let (top, count) = match pane {
        Pane::Brands => (&mut d.brand_top, d.brands.len()),
        Pane::Models => (&mut d.model_top, d.models.len()),
    };
    let max = count.saturating_sub(view::LIST_ROWS) as i32;
    *top = (*top as i32 - notches).clamp(0, max) as usize;
}

/// Model id and detected USB ids "Use" stores; none while the mice are still
/// being listed, nor on step 1 before a pick.
fn detect_choice(d: &Detect) -> Option<(String, Option<(u16, u16)>)> {
    if d.step == DetectStep::Searching {
        return None;
    }
    if d.step == DetectStep::Found && d.chip.is_none() {
        return None;
    }
    let row = d.models.get(d.model)?;
    Some((row.choice.clone(), d.chip.map(|c| d.found[c].usb)))
}

/// Identity for "Not now": VID:PID, plus the serial when the mouse has
/// one (kept to letters and digits, the config is comma-separated).
fn mouse_key(d: &DeviceInfo) -> String {
    let id = format!("{:04X}:{:04X}", d.vid, d.pid);
    let serial: String = d.serial.chars().filter(char::is_ascii_alphanumeric).collect();
    if serial.is_empty() { id } else { format!("{id}:{serial}") }
}

/// A mouse worth offering: connected since the last scan (`seen`), in the
/// catalogue with settings support (not statistics only), not the current
/// model and not dismissed. Unknown mice never are; until a mouse is
/// chosen, the first-run dialog asks anyway.
fn new_mouse<'a>(mice: &'a [DeviceInfo], seen: &[String], device: Option<&str>, ignored: &[String]) -> Option<&'a DeviceInfo> {
    let current = devices::migrate_id(device?);
    // The chosen model's own shared receiver (Bolt for an MX Master) is no news.
    let paired = |d: &DeviceInfo| devices::by_id(current).is_some_and(|c| c.on_receiver(d));
    mice.iter().find(|d| {
        let key = mouse_key(d);
        let Match::Known(m) = devices::match_device(d) else {
            return false;
        };
        m.support != Support::StatsOnly && m.id != current && !paired(d) && !seen.contains(&key) && !ignored.contains(&key)
    })
}

/// `"046D:C52B"`, as `mouse_key` starts.
fn usb_id(usb: (u16, u16)) -> String {
    format!("{:04X}:{:04X}", usb.0, usb.1)
}

/// Whether the chosen mouse is among the connected ones. A choice made
/// from the lists has no USB ids: any product of its model counts; "other
/// mouse" without ids cannot be told, so it counts as connected.
fn chosen_connected(mice: &[DeviceInfo], device: &str, usb: Option<(u16, u16)>) -> bool {
    if let Some(usb) = usb {
        return mice.iter().any(|d| (d.vid, d.pid) == usb);
    }
    if device == store::GENERIC_DEVICE {
        return true;
    }
    let id = devices::migrate_id(device);
    mice.iter().any(|d| matches!(devices::match_device(d), Match::Known(m) if m.id == id))
}

/// The remembered mouse to switch to by itself, if any:
///
/// ```text
/// a remembered mouse was connected since the last scan   -> that one
/// the chosen mouse is gone and a remembered one is there -> the last used
/// ```
/// `seen` are the keys of the last scan; `None` at start, when nothing is
/// news. Never the chosen mouse itself, and nothing before the first choice.
fn remembered<'a>(mice: &[DeviceInfo], seen: Option<&[String]>, cfg: &'a AppConfig) -> Option<&'a KnownMouse> {
    let device = cfg.device.as_deref()?;
    let chosen = |k: &KnownMouse| k.device == device && Some(k.usb) == cfg.device_usb;
    let connected = |k: &KnownMouse| mice.iter().any(|d| (d.vid, d.pid) == k.usb);
    let ready: Vec<&KnownMouse> = cfg.known_mice.iter().filter(|k| !chosen(k) && connected(k)).collect();

    let fresh = |k: &&KnownMouse| seen.is_some_and(|seen| !seen.iter().any(|key| key.starts_with(&usb_id(k.usb))));
    if let Some(k) = ready.iter().copied().find(fresh) {
        return Some(k);
    }
    if chosen_connected(mice, device, cfg.device_usb) {
        return None;
    }
    ready.first().copied()
}

/// The banner for a catalogue mouse ("Brand Model"); `None` for any other.
fn banner_of(d: &DeviceInfo) -> Option<Banner> {
    let Match::Known(m) = devices::match_device(d) else {
        return None;
    };
    let (badge, kind) = support_badge(m.support);
    Some(Banner { name: detected(d).0, badge: badge.into(), kind })
}

/// Device id "Switch" stores: what the detect dialog would.
fn offer_choice(d: &DeviceInfo) -> String {
    detected(d).1.id.clone()
}

/// Remembers a "Not now" for this mouse: it is not switched to either.
fn ignore_mouse(cfg: &mut AppConfig, d: &DeviceInfo) {
    cfg.known_mice.retain(|k| k.usb != (d.vid, d.pid));
    let key = mouse_key(d);
    if !cfg.ignored_mice.contains(&key) {
        cfg.ignored_mice.push(key);
    }
}

fn rate_index(r: PollingRate) -> usize {
    PollingRate::ALL.iter().position(|&x| x == r).unwrap_or(0)
}

/// Device to store when the dialog is cancelled. The first-run question
/// settles on "other mouse" so it isn't asked again until "Change mouse";
/// later the current choice stays (`None`). A dialog that was still looking
/// for mice has asked nothing, so it leaves the question to its answer.
fn cancel_choice(step: DetectStep, device: Option<&str>) -> Option<&'static str> {
    if step == DetectStep::Searching {
        return None;
    }
    device.is_none().then_some(store::GENERIC_DEVICE)
}

/// Default layout; the old one for Undo, `None` if nothing changed.
fn reset_layout(cfg: &mut AppConfig) -> Option<Undo> {
    if cfg.buttons == DEFAULT_ACTIONS {
        return None;
    }
    let before = std::mem::replace(&mut cfg.buttons, DEFAULT_ACTIONS);
    Some(Undo::Buttons(before))
}

/// Empties macro `i`; its steps for Undo, `None` if it was empty.
fn clear_macro(cfg: &mut AppConfig, i: usize) -> Option<Undo> {
    let m = cfg.macros.get_mut(i).filter(|m| !m.is_empty())?;
    Some(Undo::Macro(i, std::mem::take(m)))
}

/// Puts back what an undo captured.
fn apply_undo(cfg: &mut AppConfig, u: Undo) -> Resend {
    match u {
        Undo::Buttons(b) => {
            cfg.buttons = b;
            Resend::Buttons
        }
        Undo::Macro(i, steps) => {
            if let Some(m) = cfg.macros.get_mut(i) {
                *m = steps;
            }
            Resend::Macros
        }
        Undo::Profile(at, profile) => {
            profiles::restore(cfg, at, profile);
            Resend::Layout
        }
    }
}

/// Whether a dialog or picker covers the page.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cover {
    Modal,
    Free,
}

/// Stores a button layout; refused if no button would stay a left click.
/// A pending undo is stale after any other edit, so it is dropped.
fn store_layout(cfg: &mut AppConfig, undo: &mut Option<Undo>, buttons: [Action; protocol::BUTTON_COUNT]) -> Layout {
    if !buttons.contains(&Action::Mouse(MB_LEFT)) {
        return Layout::Refused;
    }
    *undo = None;
    cfg.buttons = buttons;
    Layout::Stored
}

/// Another mouse is chosen: an undo for the previous one must not reach it.
/// One picked among the connected mice (`usb`) is remembered for the
/// automatic switch, and a former "Not now" for it is over.
fn switch_device(cfg: &mut AppConfig, undo: &mut Option<Undo>, choice: String, usb: Option<(u16, u16)>) {
    *undo = None;
    if let Some(usb) = usb {
        cfg.remember_mouse(&choice, usb);
        let id = usb_id(usb);
        cfg.ignored_mice.retain(|key| !key.starts_with(&id));
    }
    cfg.device = Some(choice);
    cfg.device_usb = usb;
}

/// Ctrl+Z: puts back the pending undo; waits while a dialog is open.
fn ctrl_z(cfg: &mut AppConfig, undo: &mut Option<Undo>, cover: Cover) -> Option<Resend> {
    if cover == Cover::Modal {
        return None;
    }
    Some(apply_undo(cfg, undo.take()?))
}

/// What a pressed key does, given the focus and the acts of the last frame.
#[derive(Debug, PartialEq, Eq)]
enum Key {
    Close,
    Focus(Step),
    /// Activate this element.
    Press(Act),
    /// Enter with nothing focused: the dialog's default button.
    Default,
    Nudge(Step),
    Undo,
    Page(Page),
    Ignore,
}

fn key_action(vk: u16, m: Mods, focus: Option<Focus>, acts: &[Act], caps: &Caps, cover: Cover) -> Key {
    // A focus left over from another page or dialog is no focus.
    let focus = focus.and_then(|x| x.locate(acts)).map(|i| acts[i]);
    match vk {
        VK_ESCAPE => Key::Close,
        VK_TAB => Key::Focus(if m.shift { Step::Prev } else { Step::Next }),
        VK_RETURN | VK_SPACE => match focus {
            // Sliders move with the arrow keys instead.
            Some(Act::Slider(_) | Act::BasicDpi) => Key::Ignore,
            Some(a) => Key::Press(a),
            None if vk == VK_RETURN => Key::Default,
            None => Key::Ignore,
        },
        VK_LEFT => Key::Nudge(Step::Prev),
        VK_RIGHT => Key::Nudge(Step::Next),
        VK_Z if command_key(m) => Key::Undo,
        _ => match page_shortcut(vk, m, caps) {
            Some(p) if cover == Cover::Free => Key::Page(p),
            _ => Key::Ignore,
        },
    }
}

/// Inputs of the Mouse tab: catalogue model, USB identity, driver
/// identity, Lionel status, shown name and link state.
struct MouseFacts {
    model: Option<&'static Model>,
    info: Option<DeviceInfo>,
    ident: Option<Ident>,
    status: Option<Status>,
    name: String,
    connected: bool,
}

/// Mouse tab rows from the catalogue, USB identity and status, in the
/// current language.
fn device_rows(f: &MouseFacts) -> DeviceView {
    let model = f.model;
    let info = f.info.as_ref();
    let mut rows: Vec<(T, String)> = Vec::new();
    let mut status = String::new();
    // The mouse's own firmware, for the Overview line.
    let mut firmware = String::new();

    let name = if model.is_none() { other_name(info) } else { f.name.clone() };
    rows.push((T::DevModel, name));
    // What the mouse calls itself decides verified auto-detected models.
    let reported = f.ident.as_ref().map_or("", |i| i.name.as_str());
    let support = model.map(|m| devices::support_of(m, reported));
    if let Some(m) = model {
        rows.push((T::DevSupport, support_badge(support.unwrap_or(m.support)).0.into()));
        let link = info.and_then(|i| m.link_of(i.pid)).map(|l| match l {
            Link::Receiver => tr(T::LinkReceiver).to_string(),
            Link::Cable => tr(T::LinkCable).to_string(),
        });
        rows.push((T::DevConnection, link.unwrap_or_default()));
    }
    let ident = f.ident.as_ref().filter(|_| model.is_some_and(|m| m.family.has_driver()));
    if let Some(addr) = info.and_then(|i| i.bt_address.as_deref()) {
        // AA:BB:CC:DD:EE:FF
        let pretty: Vec<&str> = (0..addr.len()).step_by(2).map(|k| &addr[k..k + 2]).collect();
        rows.push((T::DevConnection, format!("Bluetooth LE · {}", pretty.join(":"))));
    }
    if let Some(i) = info {
        rows.push((T::DevUsbId, format!("{:04X}:{:04X}", i.vid, i.pid)));
        rows.push((T::DevManufacturer, i.manufacturer.clone()));
        rows.push((T::DevProduct, i.product.clone()));
        let own = ident.map(|d| d.firmware.clone()).filter(|f| !f.is_empty());
        // Behind a receiver, bcdDevice is the receiver's version, not the mouse's.
        let receiver = model.and_then(|m| m.link_of(i.pid)) == Some(Link::Receiver);
        let usb_fw = devices::bcd_version(i.version);
        let mouses = own.is_some() || !receiver;
        let fw = match own {
            Some(fw) => fw,
            None if receiver => fill(tr(T::ReceiverFw), &[&usb_fw]),
            None => usb_fw,
        };
        if mouses {
            firmware = fw.clone();
        }
        rows.push((T::DevFirmware, fw));
        let serial = ident.map(|d| d.serial.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| i.serial.clone());
        rows.push((T::DevSerial, serial));
    }
    if let Some(d) = ident {
        rows.push((T::DevProtocol, d.protocol.clone()));
    }
    if let Some(s) = f.status.filter(|_| model.is_some()) {
        let sensor = devices::sensor_name(s.sensor).map_or_else(|| format!("0x{:02X}", s.sensor), str::to_string);
        rows.push((T::DevSensor, sensor));
        rows.push((T::DevProtocol, "Telink · M022".into()));
        // Payload bytes of the status reply, for bug reports (Diagnostics).
        let raw: Vec<String> = s.raw[5..24].iter().map(|b| format!("{b:02X}")).collect();
        status = raw.join(" ");
    }

    // Only driver-based or Bluetooth mice have a connection state to report.
    let driven = model.is_some_and(|m| m.family != Family::NoDriver) || info.is_some_and(|i| i.bt_address.is_some());
    let offline = driven && !f.connected && info.is_some();
    let experimental = support == Some(Support::Experimental);
    DeviceView { rows, firmware, status, offline, experimental }
}

/// Rebuilds the view texts made from translations after a language switch.
fn relabel(v: &mut View, link: T, facts: &MouseFacts) {
    v.link = tr(link).into();
    v.device = device_rows(facts);
}

/// Empty wizard view at `step`.
fn report_view(step: ReportStep) -> ReportView {
    ReportView {
        kind: ReportKind::Mouse,
        step,
        rows: Vec::new(),
        sel: 0,
        lines: Vec::new(),
        top: 0,
        note: String::new(),
        note_bad: false,
        sending: Sending::Ready,
        sent: None,
        can_check: false,
        check: None,
    }
}

/// "Acme Glide" · "1234:5678 · USB · current".
fn pick_row(d: &HidDevice, current: Option<(u16, u16)>) -> PickRow {
    let link = d.collections.first().map_or("", |(_, l)| report::transport(l.transport));
    let mut sub = format!("{} · {link}", report::usb_id(&d.info));
    if current == Some((d.info.vid, d.info.pid)) {
        sub = format!("{sub} · {}", tr(T::CurrentTag));
    }
    PickRow { title: report::product(&d.info), sub }
}

/// Drivers that may know a vendor id, catalogue order, each once.
fn probe_targets(vid: u16) -> Vec<Target> {
    let drivers = devices::families_for(vid).into_iter().filter(|f| *f != Family::NoDriver);
    drivers.map(Family::target).collect()
}

/// Wheel over the wizard: the pick list moves the selection, the
/// report scrolls three lines a notch.
fn report_wheel(r: &mut ReportView, notches: i32) {
    const LINES_PER_NOTCH: i32 = 3;
    if r.step == ReportStep::Pick {
        let last = r.rows.len().saturating_sub(1) as i32;
        r.sel = (r.sel as i32 - notches).clamp(0, last.max(0)) as usize;
        return;
    }
    scroll_report(r, -notches * LINES_PER_NOTCH);
}

/// Moves the report text by `lines`, within the text.
fn scroll_report(r: &mut ReportView, lines: i32) {
    let max = r.lines.len().saturating_sub(view::REPORT_ROWS) as i32;
    r.top = (r.top as i32 + lines).clamp(0, max.max(0)) as usize;
}

/// Up / Down pick a mouse; on the report they scroll, Page Up / Down by a page.
fn report_key(r: &mut ReportView, vk: u16) -> bool {
    let page = view::REPORT_ROWS as i32;
    match (r.step, vk) {
        (ReportStep::Pick, VK_UP) => r.sel = r.sel.saturating_sub(1),
        (ReportStep::Pick, VK_DOWN) => r.sel = (r.sel + 1).min(r.rows.len().saturating_sub(1)),
        (ReportStep::Result, VK_UP) => scroll_report(r, -1),
        (ReportStep::Result, VK_DOWN) => scroll_report(r, 1),
        (ReportStep::Result, VK_PRIOR) => scroll_report(r, -page),
        (ReportStep::Result, VK_NEXT) => scroll_report(r, page),
        _ => return false,
    }
    true
}

/// Title, question, live value and answers of the open check.
fn check_view(v: &Verify, c: Check, measured: Option<u32>) -> CheckView {
    let (n, total) = v.position();
    let s = v.start();
    let (name, question, value, ask) = match c {
        Check::Dpi => {
            let q = fill(tr(T::CheckDpi), &[&v.test_dpi().to_string(), &s.dpi.unwrap_or_default().to_string()]);
            ("DPI", q, String::new(), Ask::YesNo)
        }
        Check::Rate => {
            let live = measured.map_or_else(|| tr(T::Measuring).to_string(), |hz| fill(tr(T::MeasuredValue), &[&format!("{hz} Hz")]));
            ("Hz", fill(tr(T::CheckRate), &[&v.test_rate().to_string()]), live, Ask::Measure)
        }
        Check::Battery => (tr(T::Battery), fill(tr(T::CheckBattery), &[&s.battery.unwrap_or_default().to_string()]), String::new(), Ask::YesNo),
    };
    let title = fill(tr(T::CheckOf), &[&n.to_string(), &total.to_string(), name]);
    CheckView { title, question, value, ask }
}

/// Text for "Copy for support": version, OS, the Mouse tab's rows and
/// the raw status. Pasted into public issues, so the serial number is
/// left out and a Bluetooth address keeps only its last two bytes. The
/// caller builds it in English (`i18n::with_lang`).
fn support_text(rows: &[(T, String)], status: &str, os: &str) -> String {
    let mut lines = vec![format!("{} {}", crate::APP_NAME, env!("CARGO_PKG_VERSION")), format!("OS: {os}")];
    let shared = rows.iter().filter(|(k, _)| *k != T::DevSerial);
    lines.extend(shared.map(|(k, v)| {
        let v = if v.is_empty() { tr(T::NotReported).to_string() } else { mask_address(v) };
        format!("{}: {v}", tr(*k))
    }));
    if !status.is_empty() {
        lines.push(format!("{}: {status}", tr(T::DevStatus)));
    }
    lines.join("\n")
}

/// "Bluetooth LE · B6:6B:18:D7:1E:00" -> "Bluetooth LE · XX:XX:XX:XX:1E:00";
/// other values unchanged.
fn mask_address(value: &str) -> String {
    const PARTS: usize = 6;
    const KEEP: usize = 2;
    let (head, addr) = value.rsplit_once(' ').unwrap_or(("", value));
    let parts: Vec<&str> = addr.split(':').collect();
    let is_mac = parts.len() == PARTS && parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()));
    if !is_mac {
        return value.to_string();
    }
    let masked: Vec<&str> = (0..PARTS).map(|i| if i < PARTS - KEEP { "XX" } else { parts[i] }).collect();
    let sep = if head.is_empty() { "" } else { " " };
    format!("{head}{sep}{}", masked.join(":"))
}

/// Ctrl is the shortcut modifier.
fn command_key(m: Mods) -> bool {
    m.ctrl
}

/// Ctrl+1..7 opens the nth page of the sidebar, Ctrl+, Settings.
fn page_shortcut(vk: u16, m: Mods, caps: &Caps) -> Option<Page> {
    if !command_key(m) {
        return None;
    }
    if vk == VK_COMMA {
        return Some(Page::General);
    }
    let n = vk.checked_sub(VK_1)? as usize;
    view::nav_pages(caps).get(n).copied()
}

/// Next or previous clickable element, wrapping; the first Tab starts at
/// the beginning (Shift+Tab at the end).
fn next_focus(acts: &[Act], cur: Option<Focus>, step: Step) -> Option<Focus> {
    let n = acts.len();
    if n == 0 {
        return None;
    }
    let j = match (cur.and_then(|f| f.locate(acts)), step) {
        (None, Step::Next) => 0,
        (None, Step::Prev) => n - 1,
        (Some(i), Step::Next) => (i + 1) % n,
        (Some(i), Step::Prev) => (i + n - 1) % n,
    };
    Some(Focus { index: j, act: acts[j] })
}

/// Opacity of the recording dot `e` into the recording: full at the
/// start of each period, down to a third halfway.
fn pulse_alpha(e: Duration) -> u8 {
    const LOW: f32 = 0.33;
    let phase = (e.as_secs_f32() / PULSE_PERIOD.as_secs_f32()).fract();
    let wave = 0.5 + 0.5 * (phase * std::f32::consts::TAU).cos();
    ((LOW + (1.0 - LOW) * wave) * u8::MAX as f32) as u8
}

/// Opacity of "Saved to mouse" `e` after the confirmation; `None` once gone.
fn fade_alpha(e: Duration) -> Option<u8> {
    let Some(fading) = e.checked_sub(SAVED_HOLD) else {
        return Some(u8::MAX);
    };
    let left = 1.0 - fading.as_secs_f32() / SAVED_FADE.as_secs_f32();
    (left > 0.0).then(|| (left * u8::MAX as f32) as u8)
}

// ---- window hooks ----------------------------------------------------------

fn hook_draw(g: &mut Gfx, hot: Option<Act>) -> Vec<Hit> {
    with_ctrl(|c| {
        let hits = view::draw(g, &c.view, hot);
        // Tab order: each act once, in drawing order.
        c.acts.clear();
        for h in &hits {
            c.acts.push(h.act);
        }
        hits
    })
    .unwrap_or_default()
}

/// A click: the keyboard focus ring goes away.
fn hook_act(a: Act) {
    with_ctrl(|c| {
        c.view.focus = None;
        c.on_act(a);
    });
}

/// Mouse wheel: scrolls a list of the detect dialog, the only scrolling place.
fn hook_wheel(x: f32, y: f32, notches: i32) {
    with_ctrl(|c| {
        if let Some(r) = c.view.report.as_mut().filter(|_| view::report_at(x, y)) {
            report_wheel(r, notches);
            window::invalidate();
            return;
        }
        // Only step 2 has lists.
        let Some(d) = c.view.detect.as_mut().filter(|d| d.step == DetectStep::List) else {
            return;
        };
        if let Some(pane) = view::detect_pane_at(x, y) {
            detect_wheel(d, pane, notches);
            window::invalidate();
        }
    });
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
        TIMER_REFRESH => {
            c.refresh_stats();
        }
        TIMER_SAVE if !c.removed => c.stats.save(),
        TIMER_TOAST => c.hide_toast(),
        TIMER_FADE => c.fade_sync(),
        TIMER_PULSE => c.pulse(),
        TIMER_SCAN => c.scan(),
        TIMER_UPDATE => c.auto_check(),
        TIMER_FX => c.animate(),
        _ => {}
    });
}

fn hook_tray(a: TrayAction) {
    match a {
        TrayAction::Open => {
            with_ctrl(|c| c.show());
        }
        TrayAction::About => {
            with_ctrl(|c| {
                c.show();
                c.on_act(Act::About);
            });
        }
        TrayAction::Quit => window::quit(),
        TrayAction::Profile(i) => {
            with_ctrl(|c| {
                crate::log!("tray profile {i}");
                c.end_rename();
                c.switch_profile(i);
                // The menu's mark follows even when nothing changed.
                c.sync_profiles();
                window::invalidate();
            });
        }
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
        self.visible = true;
        window::show();
        self.mouse.send(Command::Visible(true));
        self.mouse.send(Command::RefreshNow);
        self.refresh_stats();
        window::set_timer(TIMER_REFRESH, REFRESH_MS);
        self.sync_fx_timer();
    }

    /// The animation timer runs only while there is something to animate
    /// and someone to see it.
    fn sync_fx_timer(&mut self) {
        if self.visible && self.cfg.fx_motion {
            window::set_timer(TIMER_FX, FX_TICK_MS);
            return;
        }
        window::kill_timer(TIMER_FX);
        self.view.status_pulse = u8::MAX;
    }

    /// One animation frame: the connection mark breathes. Nothing moves
    /// while the window is not the one the user works in.
    fn animate(&mut self) {
        const LOW: f32 = 0.35;
        if !window::is_active() || !self.view.connected {
            return;
        }
        let phase = (self.started.elapsed().as_secs_f32() / STATUS_PULSE.as_secs_f32()).fract();
        let wave = 0.5 + 0.5 * (phase * std::f32::consts::TAU).cos();
        self.view.status_pulse = ((LOW + (1.0 - LOW) * wave) * u8::MAX as f32) as u8;
        window::invalidate();
    }

    /// Graphical or text mode; remembered like the theme.
    fn toggle_tui(&mut self) {
        self.cfg.tui = !self.cfg.tui;
        self.view.tui = self.cfg.tui;
        self.view.focus = None;
        store::save_config(&self.cfg);
    }

    fn toggle_fx(&mut self, a: Act) {
        match a {
            Act::FxTexture => self.cfg.fx_texture = !self.cfg.fx_texture,
            _ => self.cfg.fx_motion = !self.cfg.fx_motion,
        }
        (self.view.fx_texture, self.view.fx_motion) = (self.cfg.fx_texture, self.cfg.fx_motion);
        store::save_config(&self.cfg);
        self.sync_fx_timer();
    }

    /// Hidden window: slow polling, no UI refresh timer.
    fn on_hidden(&mut self) {
        self.visible = false;
        // Checks change settings: never leave them changed out of sight.
        self.cancel_checks();
        self.on_focus_lost();
        window::kill_timer(TIMER_REFRESH);
        window::kill_timer(TIMER_FX);
        self.mouse.send(Command::Visible(false));
        if !self.removed {
            self.stats.save();
        }
    }

    /// Keyboard input stops reaching us: end recording and key capture.
    fn on_focus_lost(&mut self) {
        self.end_rename();
        self.stop_recording();
        if let Some(p) = self.view.picker.as_mut() {
            p.capturing = false;
        }
        window::invalidate();
    }

    fn on_act(&mut self, a: Act) {
        let before = self.overlay();
        crate::log!("act {a:?}");
        // A press ends typing a profile name and keeps what was typed;
        // on "Rename" itself that is all it does.
        if self.end_rename() && a == Act::ProfileRename {
            window::invalidate();
            return;
        }
        self.act(a);
        // Any other press closes the open language list.
        if a != Act::LangMenu {
            self.view.lang_menu = false;
        }
        // Page or dialog changed: the focused element is gone.
        if matches!(a, Act::Nav(_)) || self.overlay() != before {
            self.view.focus = None;
        }
        window::invalidate();
    }

    /// Which dialogs are open, to notice one opening or closing.
    fn overlay(&self) -> (bool, bool, bool, Option<Dialog>, Option<ReportStep>) {
        let report = self.view.report.as_ref().map(|r| r.step);
        (self.view.ask_setup, self.view.detect.is_some(), self.view.picker.is_some(), self.view.dialog, report)
    }

    fn act(&mut self, a: Act) {
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
                self.send_setting(Command::SetDebounce(self.cfg.debounce));
                store::save_config(&self.cfg);
            }
            Act::EditButton(i) => {
                self.view.notice.clear();
                self.view.picker = Some(Picker::for_button(i, self.cfg.buttons[i]));
            }
            Act::ResetButtons => self.reset_buttons(),
            Act::PickTab(_) | Act::PickMod(_) | Act::PickCapture | Act::PickInterval(_) | Act::PickLoops(_) => self.edit_picker(a),
            Act::PickMacro(_) | Act::PickMode(_) | Act::PickAction(_) => self.edit_picker(a),
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
                    self.drop_undo();
                    self.cfg.macros[self.view.macro_sel].clear();
                    self.view.macros = self.cfg.macros.clone();
                    self.view.recording = true;
                    self.last_step = None;
                    self.rec_start = Instant::now();
                    self.view.pulse = u8::MAX;
                    window::set_timer(TIMER_PULSE, PULSE_TICK_MS);
                }
            }
            Act::MacroClear => self.clear_selected_macro(),
            Act::MacroDelay(d) => {
                self.view.delay_mode = d;
                // A fixed delay also rewrites the steps already recorded.
                if let DelayMode::Fixed(ms) = d {
                    self.cfg.macros[self.view.macro_sel].iter_mut().for_each(|e| e.delay_ms = ms);
                    self.macros_changed();
                }
            }
            Act::Profile(_) | Act::ProfileNew | Act::ProfileDuplicate | Act::ProfileRename | Act::ProfileDelete => self.profile_act(a),
            Act::ProfileExport | Act::ProfileExportAll | Act::ProfileImport => self.profile_act(a),
            Act::ChangeMouse => self.open_detect_scan(),
            Act::BannerSwitch => self.switch_to_offer(),
            Act::BannerDismiss => self.ignore_offer(),
            Act::BasicDpi => {}
            Act::BasicRate(hz) => {
                self.view.basic.rate = Some(hz);
                self.send_setting(Command::SetBasicRate(hz));
            }
            Act::DetectSel(_) | Act::DetectBrand(_) | Act::DetectFound(_) | Act::DetectBack | Act::DetectList => self.detect_pick(a),
            Act::DetectUse => self.choose_device(),
            Act::DetectCancel => self.cancel_detect(),
            Act::Lod(lod) => {
                self.cfg.lod = lod;
                self.view.lod = lod;
                self.send_setting(Command::SetLod(lod));
                store::save_config(&self.cfg);
            }
            Act::Undo => {
                self.undo();
            }
            Act::About => {
                self.stop_recording();
                self.view.page = Page::About;
            }
            Act::AssignMacro => self.view.dialog = Some(Dialog::AssignMacro),
            Act::StageNudge(by) => {
                let i = self.view.active;
                let last = protocol::dpi_steps().saturating_sub(1) as i64;
                let index = self.cfg.dpi.stages.get(i).map(|&at| (at as i64 + by as i64).clamp(0, last) as usize);
                if let Some(index) = index {
                    self.set_stage(i, index);
                }
            }
            Act::CloseDialog => self.view.dialog = None,
            // Moving the window is the platform's business; a mock-up has none.
            Act::Caption | Act::Mock(_) => {}
            Act::FxTexture | Act::FxMotion => self.toggle_fx(a),
            Act::ToggleTui => self.toggle_tui(),
            Act::LangMenu => self.view.lang_menu = !self.view.lang_menu,
            Act::WinMinimize => window::minimize(),
            // Closing hides: the app stays in the tray.
            Act::WinClose => window::close(),
            Act::ShowLog => self.show_log(),
            Act::AssignTo(b) => self.assign_macro(b),
            Act::OpenWeb(w) => {
                let url = match w {
                    Web::Site => SITE_URL,
                    Web::Repo => REPO_URL,
                };
                platform::open_url(url);
            }
            Act::Diagnostics => self.view.diagnostics = !self.view.diagnostics,
            Act::CopyDiagnostics => self.copy_diagnostics(),
            Act::CopyCommand => self.copy_command(),
            Act::ReportOpen => self.open_report(),
            Act::ReportSel(i) => {
                if let Some(r) = self.view.report.as_mut() {
                    r.sel = i.min(r.rows.len().saturating_sub(1));
                }
            }
            Act::ReportRead => self.read_report(),
            Act::ReportCopy => self.copy_report(),
            Act::ReportSend => self.send_report(),
            Act::ReportCopyId => self.copy_report_id(),
            Act::ReportIssue => self.open_issue(),
            Act::ReportCheck => self.start_checks(),
            Act::ReportAnswer(a) => self.answer_check(a),
            Act::ReportCancelChecks => self.cancel_checks(),
            Act::ReportClose => self.close_report(),
            Act::UpdateShow => self.show_update(),
            Act::UpdateCheck => self.start_check(Trigger::Manual),
            Act::UpdateAuto => self.toggle_update_auto(),
            Act::UpdateInstall => self.install_update(),
            Act::UpdateLater => self.view.dialog = None,
            Act::UpdateSkip => self.skip_update(),
            Act::UpdatePage => {
                platform::open_url(SITE_URL);
            }
        }
    }

    // ---- device status ----

    fn on_mouse(&mut self, ev: Event) {
        if !belongs_to(&ev, &self.target) {
            return;
        }
        #[cfg(debug_assertions)]
        let ev = demo_status(ev, self.view.active);
        let (link, level) = match ev {
            Event::Device(info) => {
                self.device_info = Some(info);
                self.view.access = Access::Granted;
                self.refresh_device();
                return;
            }
            Event::Ident(ident, ability) => {
                self.on_ident(ident, ability);
                return;
            }
            Event::Probe(pick, p) => {
                self.on_probe(&pick, p);
                return;
            }
            Event::Basic(r) => {
                self.basic_online = true;
                self.check_restore(r.dpi, r.rate_hz);
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
                self.view.battery_approx = r.battery_approx;
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
                self.view.access = self.hid_access();
                (T::LinkNoReceiver, Level::Offline)
            }
        };

        self.view.connected = self.status.is_some() || self.basic_online;
        if self.view.connected {
            self.mark_saved();
        }
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
                let base = format!("{} · {}", crate::APP_NAME, fill(tr(T::TipBattery), &[&approx_pct(pct, self.view.battery_approx)]));
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
        self.view.battery_approx = false;
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
        self.send_setting(Command::SetDpi(self.cfg.dpi.clone()));
        store::save_config(&self.cfg);
    }

    /// Single-DPI slider: preview while dragging, send on release.
    fn drag_basic_dpi(&mut self, index: usize, phase: Phase) {
        let Some(&dpi) = self.view.basic.values.get(index) else {
            return;
        };
        self.view.basic.dpi = Some(dpi);
        if phase == Phase::Done {
            self.send_setting(Command::SetBasicDpi(dpi));
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

    /// Whether the mouse's HID devices refuse this user (Linux without
    /// the udev rule); only mice opened over HID can be refused.
    fn hid_access(&self) -> Access {
        let vid = match self.target {
            Target::Bluetooth(_) | Target::Nothing => return Access::Granted,
            Target::Lionel => self.pick().vid.unwrap_or(protocol::VENDOR_ID),
            _ => match self.pick().vid {
                Some(v) => v,
                None => return Access::Granted,
            },
        };
        hid::access(vid)
    }

    /// USB devices the driver may open: the chosen model's, or exactly the
    /// detected mouse the user picked.
    fn pick(&self) -> Pick {
        pick_for(self.cfg.device.as_deref(), self.cfg.device_usb)
    }

    fn retarget(&mut self, t: Target) {
        self.target = t.clone();
        self.mouse.send(Command::Target(t, self.pick()));
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
        self.name_from(&ident);
        self.view.caps = narrow_caps(caps_of(self.cfg.device.as_deref()), &ability);
        self.view.basic.values = ability.dpi.map(|r| r.values()).unwrap_or_default();
        self.view.basic.rates = ability.rates;
        self.view.basic.stages = ability.stages;
        self.view.has_battery = ability.battery;
        if !view::page_visible(self.view.page, &self.view.caps) {
            self.view.page = Page::Overview;
        }
        self.ident = Some(ident);
        self.sync_profiles();
        self.refresh_device();
    }

    /// "Other <brand> mouse": the driver names the model.
    fn name_from(&mut self, ident: &Ident) {
        let Some(m) = self.cfg.device.as_deref().and_then(devices::by_id) else {
            return;
        };
        if m.entry != Entry::Auto || ident.name.is_empty() {
            return;
        }
        self.view.device_name = auto_name(m, &ident.name);
    }

    fn on_mice(&mut self, mice: Vec<DeviceInfo>) {
        // Mice connected at start are no news.
        self.seen = Some(mice.iter().map(mouse_key).collect());
        self.mice = mice.clone();
        // The chosen mouse is not there, a remembered one is.
        if self.auto_switch(&mice, None) {
            return;
        }
        self.retarget_bluetooth();
        self.refresh_device();
        if self.cfg.device.is_none() {
            self.open_detect(mice);
        }
        #[cfg(debug_assertions)]
        self.debug_open();
    }

    /// Debug builds: `WHISKER_OPEN=detect | detect-pick | detect-list | detect-none
    /// | detect-search | detect-refetch` opens the "which mouse" dialog at
    /// start with sample mice, `dpi` the DPI page, `log` the diagnostic log,
    /// `report` a sample mouse report; for screenshots without clicking.
    /// `panic | fault | hang` fail on purpose, to try the crash notes.
    #[cfg(debug_assertions)]
    fn debug_open(&mut self) {
        const OPEN_VAR: &str = "WHISKER_OPEN";
        /// Longer than the watch's limit, so the hang is logged.
        const HANG: Duration = Duration::from_secs(8);
        static ONCE: std::sync::Once = std::sync::Once::new();
        let Ok(what) = std::env::var(OPEN_VAR) else {
            return;
        };
        match what.as_str() {
            "panic" => panic!("{OPEN_VAR}=panic"),
            // SAFETY: none, on purpose: address 8 is never mapped.
            "fault" => unsafe { (8 as *mut u8).write_volatile(1) },
            "hang" => ONCE.call_once(|| std::thread::sleep(HANG)),
            "log" => ONCE.call_once(|| self.on_act(Act::ShowLog)),
            // A mouse report as after reading, with every button offered;
            // `report-sent` as after sending it.
            "report" | "report-sent" => ONCE.call_once(|| {
                let features = "00:0000, 01:0001, 02:0003, 03:0005, 04:1D4B, 05:0020, 06:1000, 07:1B04, 08:2205, 09:2100, 0A:00C2, 0B:1802, 0C:1803";
                let text = format!("### Mouse report: sample (1234:5678)

- Whisker on a sample system
  - features (index:id): {features}");
                let mut r = report_view(ReportStep::Result);
                (r.lines, r.can_check) = (view::report_lines(&text), true);
                if what == "report-sent" {
                    (r.sending, r.sent, r.note) = (Sending::Done, Some("WR-7K3F9Q".into()), tr(T::Copied).into());
                }
                self.wizard = Some(Wizard { text, ..Wizard::default() });
                self.view.report = Some(r);
            }),
            _ => {}
        }
        if what == "dpi" {
            self.on_act(Act::Nav(Page::Dpi));
            return;
        }
        let sample = |m: &Model, product: &str| DeviceInfo { vid: m.vid, pid: m.pids[0].0, product: product.into(), ..DeviceInfo::default() };
        let unknown = DeviceInfo { vid: 0x1234, pid: 1, product: "USB Optical Mouse".into(), ..DeviceInfo::default() };
        let mut mice = vec![sample(devices::default_model(), "Lionel"), unknown];
        if let Some(katar) = devices::by_id("corsair/katar-pro") {
            mice.insert(1, sample(katar, ""));
        }
        if what == "detect-none" {
            mice.clear();
        }
        ONCE.call_once(|| {
            if what == "detect-search" {
                self.open_searching();
                return;
            }
            self.open_detect(mice);
            let Some(d) = self.view.detect.as_mut() else {
                return;
            };
            match what.as_str() {
                "detect-list" => show_list(d),
                "detect-pick" => pick_found(d, 0),
                "detect-refetch" => d.refetching = true,
                _ => {}
            }
        });
    }

    // ---- new mouse ----

    /// Lists the connected mice off the UI thread; `on_scan` gets them.
    /// Enumeration opens every HID collection, which takes seconds: it is
    /// never done on the UI thread, where the window would stop answering.
    fn scan(&mut self) {
        if self.scanning || self.removed {
            return;
        }
        self.scanning = true;
        std::thread::spawn(|| {
            let mice = hid_link::list_mice();
            post(move |c| c.on_scan(mice));
        });
    }

    /// Keeps the mouse list the controller works from: offers a catalogue
    /// mouse connected since the last scan, drops the banner when its mouse
    /// is unplugged again, and re-fills an open detect dialog.
    fn on_scan(&mut self, mice: Vec<DeviceInfo>) {
        self.scanning = false;
        let keys: Vec<String> = mice.iter().map(mouse_key).collect();
        if self.offer.as_ref().is_some_and(|o| !keys.contains(&mouse_key(o))) {
            self.close_offer();
        }
        // A dialog waiting for an answer – still looking for mice, or showing
        // the last list while a scan runs – takes it even when the list is
        // the same as the one it opened with.
        let waiting = self.view.detect.as_ref().is_some_and(|d| d.refetching || d.step == DetectStep::Searching);
        if waiting || mice != self.mice {
            self.mice = mice.clone();
            self.refresh_detect(&mice);
        }
        let Some(seen) = self.seen.replace(keys) else {
            return;
        };
        if self.auto_switch(&mice, Some(&seen)) {
            return;
        }
        let found = new_mouse(&mice, &seen, self.cfg.device.as_deref(), &self.cfg.ignored_mice).cloned();
        if let Some(d) = found {
            self.offer_mouse(d);
        }
    }

    /// Banner in the window; a system notification while it is hidden
    /// (a click on it opens the window with the banner).
    fn offer_mouse(&mut self, d: DeviceInfo) {
        let Some(banner) = banner_of(&d) else {
            return;
        };
        if !self.visible {
            let what = format!("{} ({})", banner.name, banner.badge);
            window::notify(tr(T::NewMouseTitle), &fill(tr(T::NewMouseTip), &[&what]));
        }
        self.view.banner = Some(banner);
        self.offer = Some(d);
        window::invalidate();
    }

    fn close_offer(&mut self) {
        self.offer = None;
        self.view.banner = None;
        window::invalidate();
    }

    /// "Switch": the same as picking the mouse in the detect dialog.
    fn switch_to_offer(&mut self) {
        let Some(d) = self.offer.take() else {
            return;
        };
        self.close_offer();
        self.view.detect = None;
        let (choice, usb) = (offer_choice(&d), Some((d.vid, d.pid)));
        // No-driver models are found among the detected mice.
        if !self.mice.contains(&d) {
            self.mice.push(d);
        }
        self.apply_device(choice, usb);
    }

    /// "Not now": this mouse is never offered again.
    fn ignore_offer(&mut self) {
        let Some(d) = self.offer.take() else {
            return;
        };
        self.close_offer();
        ignore_mouse(&mut self.cfg, &d);
        store::save_config(&self.cfg);
    }

    /// "Change mouse" answers the click at once and starts a fresh scan:
    /// listing the mice takes longer than the window may wait for an answer,
    /// so it never runs inside the click handler. A session that has already
    /// read a list (`seen`) shows it right away and says the newer one is on
    /// its way; before that the dialog says it is looking for mice.
    fn open_detect_scan(&mut self) {
        if self.seen.is_some() {
            self.open_refetching();
        } else {
            self.open_searching();
        }
        self.scan();
    }

    /// The dialog with the mice of the last scan, marked as the last list
    /// while `scan` brings the new one.
    fn open_refetching(&mut self) {
        self.open_detect(self.mice.clone());
        if let Some(d) = self.view.detect.as_mut() {
            d.refetching = true;
        }
    }

    /// The dialog for the mice that are still being listed on their thread.
    fn open_searching(&mut self) {
        self.view.detect = Some(searching_state(self.cfg.device.as_deref()));
        self.view.focus = None;
        window::invalidate();
    }

    /// A scan found other mice while the dialog is open; a closed dialog
    /// needs nothing.
    fn refresh_detect(&mut self, mice: &[DeviceInfo]) {
        // Demo mice are made up: a real listing must not replace them.
        if self.view.detect.is_none() || platform::demo() {
            return;
        }
        let (device, usb) = (self.cfg.device.clone(), self.cfg.device_usb);
        let reported = self.ident.as_ref().map_or(String::new(), |i| i.name.clone());
        let Some(d) = self.view.detect.as_mut() else {
            return;
        };
        merge_detect(d, mice, device.as_deref(), usb, &reported);
        window::invalidate();
    }

    fn open_detect(&mut self, mice: Vec<DeviceInfo>) {
        self.mice = mice.clone();
        let reported = self.ident.as_ref().map_or("", |i| i.name.as_str());
        self.view.detect = Some(detect_state(&mice, self.cfg.device.as_deref(), self.cfg.device_usb, reported));
        self.view.focus = None;
        window::invalidate();
    }

    /// A click in the detect dialog: model, brand, detected mouse, or a
    /// step change ("My mouse isn't listed", "Back").
    fn detect_pick(&mut self, a: Act) {
        let Some(d) = self.view.detect.as_mut() else {
            return;
        };
        d.keys = false;
        match a {
            Act::DetectSel(k) => pick_model(d, k),
            Act::DetectBrand(b) => pick_brand(d, b),
            Act::DetectFound(i) => pick_found(d, i),
            Act::DetectList => show_list(d),
            _ => {
                step_back(d);
            }
        }
        // A step change drops the focused element with its step.
        if matches!(a, Act::DetectList | Act::DetectBack) {
            self.view.focus = None;
        }
    }

    /// "Use": stays open while step 1 has no pick (Enter).
    fn choose_device(&mut self) {
        let Some((choice, usb)) = self.view.detect.as_ref().and_then(detect_choice) else {
            return;
        };
        self.view.detect = None;
        self.apply_device(choice, usb);
    }

    /// Cancel or Esc: nothing changes, except that the first-run question
    /// counts as answered with "other mouse". A dialog that was still looking
    /// for mice leaves the question to the list that is on its way.
    fn cancel_detect(&mut self) {
        let Some(step) = self.view.detect.take().map(|d| d.step) else {
            return;
        };
        if let Some(id) = cancel_choice(step, self.cfg.device.as_deref()) {
            self.apply_device(id.to_string(), None);
        }
    }

    /// Stores the chosen mouse (and which detected device, if picked from
    /// the detected ones) and points the driver at it.
    fn apply_device(&mut self, choice: String, usb: Option<(u16, u16)>) {
        self.end_rename();
        switch_device(&mut self.cfg, &mut self.undo, choice, usb);
        // The mouse has its own profiles; its active one was sent to it
        // when it was last used, so nothing is sent now.
        bind_profiles(&mut self.cfg);
        (self.view.buttons, self.view.macros) = (self.cfg.buttons, self.cfg.macros.clone());
        self.hide_toast();
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
            self.view.page = Page::Overview;
        }
        self.sync_profiles();
        self.refresh_device();
    }

    /// A remembered mouse was connected, or the chosen one is gone and a
    /// remembered one is there: it becomes the chosen mouse. Not while the
    /// user is choosing a mouse or reading one for a report. `true` if it
    /// switched.
    fn auto_switch(&mut self, mice: &[DeviceInfo], seen: Option<&[String]>) -> bool {
        if self.view.detect.is_some() || self.wizard.is_some() || platform::demo() {
            return false;
        }
        let Some(k) = remembered(mice, seen, &self.cfg).cloned() else {
            return false;
        };
        crate::log!("remembered mouse {} {}: switching", usb_id(k.usb), k.device);
        self.close_offer();
        self.apply_device(k.device, Some(k.usb));
        self.show_toast(fill(tr(T::SwitchedTo), &[&self.view.device_name]), None);
        true
    }

    /// Rows of the Device page from the catalogue, USB identity and status.
    fn refresh_device(&mut self) {
        // "Other mouse": count only that mouse, not every mouse connected,
        // and show the name it reports.
        if self.cfg.device.as_deref().and_then(devices::by_id).is_none() {
            let info = self.chosen_detected().cloned();
            self.stats.set_vendor(info.as_ref().map(|i| i.vid));
            self.view.device_name = other_name(info.as_ref());
        }
        self.view.device = self.device_view();
        window::invalidate();
    }

    /// What the Mouse tab is built from.
    fn facts(&self) -> MouseFacts {
        let model = self.cfg.device.as_deref().and_then(devices::by_id);
        // With a driver: the opened interface; otherwise the detected entry.
        let info = match model {
            Some(m) if m.family != Family::NoDriver => self.device_info.clone(),
            _ => self.chosen_detected().cloned(),
        };
        let name = self.view.device_name.clone();
        MouseFacts { model, info, ident: self.ident.clone(), status: self.status, name, connected: self.view.connected }
    }

    fn device_view(&self) -> DeviceView {
        device_rows(&self.facts())
    }

    // ---- "help add your mouse" ----

    /// Lists the connected mice off the UI thread; `on_report_mice` gets them.
    fn open_report(&mut self) {
        self.close_report();
        self.wizard = Some(Wizard::default());
        self.view.report = Some(report_view(ReportStep::Finding));
        std::thread::spawn(|| {
            let devices = hid_link::mouse_devices();
            post(move |c| c.on_report_mice(devices));
        });
    }

    /// Pick list; the mouse in use preselected.
    fn on_report_mice(&mut self, devices: Vec<HidDevice>) {
        let current = self.device_info.as_ref().map(|d| (d.vid, d.pid));
        let (Some(w), Some(r)) = (self.wizard.as_mut(), self.view.report.as_mut()) else {
            return;
        };
        if r.step != ReportStep::Finding {
            return;
        }
        r.rows = devices.iter().map(|d| pick_row(d, current)).collect();
        r.sel = devices.iter().position(|d| Some((d.info.vid, d.info.pid)) == current).unwrap_or(0);
        r.step = ReportStep::Pick;
        w.devices = devices;
        window::invalidate();
    }

    /// Read-only probe of the picked mouse on the mouse thread.
    fn read_report(&mut self) {
        let (Some(w), Some(r)) = (self.wizard.as_ref(), self.view.report.as_mut()) else {
            return;
        };
        let Some(d) = w.devices.get(r.sel).map(|d| &d.info) else {
            return;
        };
        r.step = ReportStep::Reading;
        let pick = Pick { vid: Some(d.vid), pids: vec![d.pid] };
        self.mouse.send(Command::Probe(probe_targets(d.vid), pick));
    }

    /// Probe results; a late answer for an earlier pick is dropped.
    fn on_probe(&mut self, pick: &Pick, probes: Vec<Probe>) {
        let Some(r) = self.view.report.as_ref().filter(|r| r.step == ReportStep::Reading) else {
            return;
        };
        let sel = r.sel;
        let Some(w) = self.wizard.as_mut() else {
            return;
        };
        let asked = w.devices.get(sel).is_some_and(|d| pick.vid == Some(d.info.vid) && pick.pids == [d.info.pid]);
        if !asked {
            return;
        }
        w.probes = probes;
        self.render_report();
        window::invalidate();
    }

    /// Report text (English) and the result step.
    fn render_report(&mut self) {
        let os = platform::os_version();
        let sel = self.view.report.as_ref().map_or(0, |r| r.sel);
        let Some(info) = self.wizard.as_ref().and_then(|w| w.devices.get(sel)).map(|d| d.info.clone()) else {
            return;
        };
        let checkable = self.can_check(&info);
        let Some(w) = self.wizard.as_mut() else {
            return;
        };
        let Some(dev) = w.devices.get(sel) else {
            return;
        };
        let input = report::Input { dev, probes: &w.probes, checks: &w.checks, os: &os };
        let text = i18n::with_lang(Lang::En, || report::text(&input));
        let lines = view::report_lines(&text);
        let checkable = checkable && w.checks.is_empty();
        w.text = text;
        let Some(r) = self.view.report.as_mut() else {
            return;
        };
        (r.lines, r.top, r.step, r.can_check, r.check) = (lines, 0, ReportStep::Result, checkable, None);
        // A new text (the checks added their results) can be sent again.
        r.sending = Sending::Ready;
    }

    /// Guided checks: the mouse in use, driven by a driver not yet
    /// verified, with at least one value that can be checked.
    fn can_check(&self, d: &DeviceInfo) -> bool {
        let Some(m) = self.cfg.device.as_deref().and_then(devices::by_id) else {
            return false;
        };
        let same = self.device_info.as_ref().is_some_and(|i| (i.vid, i.pid) == (d.vid, d.pid));
        let drivable = m.family.has_driver() && m.support != Support::Full && self.basic_online && same;
        drivable && Verify::new(self.check_start()).is_some()
    }

    /// What the checks start from: the last reading and abilities.
    fn check_start(&self) -> Start {
        let b = &self.view.basic;
        let battery = self.view.has_battery.then_some(self.view.battery);
        Start { dpi: b.dpi, values: b.values.clone(), rate: b.rate, rates: b.rates.clone(), battery }
    }

    fn copy_report(&mut self) {
        let Some(text) = self.wizard.as_ref().map(|w| w.text.clone()) else {
            return;
        };
        if window::set_clipboard(&text) {
            self.report_note(tr(T::Copied));
        }
    }

    /// New-issue page with the report, or copied for pasting when too long.
    fn open_issue(&mut self) {
        let sel = self.view.report.as_ref().map_or(0, |r| r.sel);
        let Some(w) = self.wizard.as_ref() else {
            return;
        };
        let Some(dev) = w.devices.get(sel) else {
            return;
        };
        let text = w.text.clone();
        match report::issue(NEW_ISSUE_URL, &report::title(&report::public(&dev.info)), &text) {
            report::Issue::Prefilled(url) => {
                platform::open_url(&url);
                self.report_note(tr(T::OpenedInBrowser));
            }
            report::Issue::Paste(url) => {
                window::set_clipboard(&text);
                platform::open_url(&url);
                self.report_note(tr(T::ReportPasteNote));
            }
        }
    }

    fn report_note(&mut self, note: &str) {
        if let Some(r) = self.view.report.as_mut() {
            (r.note, r.note_bad) = (note.to_string(), false);
        }
    }

    /// The diagnostic log in the report view: read first, then copy or send.
    fn show_log(&mut self) {
        self.view.dialog = None;
        self.close_report();
        let text = crate::log::text();
        let mut r = report_view(ReportStep::Result);
        r.kind = ReportKind::Log;
        r.lines = view::report_lines(&text);
        // The newest lines are what the notice is about.
        r.top = r.lines.len().saturating_sub(view::REPORT_ROWS);
        self.wizard = Some(Wizard { text, ..Wizard::default() });
        self.view.report = Some(r);
    }

    /// "Send": the text as shown goes to the developers, off the UI thread.
    fn send_report(&mut self) {
        let (Some(w), Some(r)) = (self.wizard.as_mut(), self.view.report.as_mut()) else {
            return;
        };
        if r.sending != Sending::Ready || self.send_busy {
            return;
        }
        self.send_seq += 1;
        let seq = self.send_seq;
        (self.send_busy, w.send) = (true, Some(seq));
        let kind = match r.kind {
            ReportKind::Mouse => feedback::Kind::Mouse,
            ReportKind::Log => feedback::Kind::Log,
        };
        (r.sending, r.note, r.note_bad) = (Sending::Busy, tr(T::Sending).to_string(), false);

        let text = w.text.clone();
        std::thread::spawn(move || {
            let sent = feedback::send(kind, &text);
            post(move |c| c.on_sent(seq, sent));
        });
    }

    /// The service's answer to send number `seq`: the report id to quote
    /// (also in the log, should the dialog be gone), or a failure.
    fn on_sent(&mut self, seq: u64, sent: Result<String, feedback::Fail>) {
        self.send_busy = false;
        match &sent {
            Ok(id) => crate::log!("report sent: {id}"),
            Err(e) => crate::log!("report not sent: {e:?}"),
        }

        // The dialog it was sent from was closed (or is another one by
        // now): the answer still reaches the user.
        let same = self.wizard.as_ref().is_some_and(|w| w.send == Some(seq));
        let Some(r) = self.view.report.as_mut().filter(|_| same) else {
            let note = match &sent {
                Ok(id) => fill(tr(T::Sent), &[id]),
                Err(_) => tr(T::SendFailed).to_string(),
            };
            self.show_toast(note, None);
            return;
        };
        match sent {
            Ok(id) => (r.sending, r.sent, r.note, r.note_bad) = (Sending::Done, Some(id), String::new(), false),
            Err(_) => (r.sending, r.note, r.note_bad) = (Sending::Ready, tr(T::SendFailed).to_string(), true),
        }
        window::invalidate();
    }

    fn copy_report_id(&mut self) {
        let Some(id) = self.view.report.as_ref().and_then(|r| r.sent.clone()) else {
            return;
        };
        if window::set_clipboard(&id) {
            self.report_note(tr(T::Copied));
        }
    }

    /// First check: its setting is changed now.
    fn start_checks(&mut self) {
        let start = self.check_start();
        let (Some(w), Some(mut v)) = (self.wizard.as_mut(), Verify::new(start)) else {
            return;
        };
        v.begin(&mut self.mouse);
        w.verify = Some(v);
        if let Some(r) = self.view.report.as_mut() {
            (r.step, r.note) = (ReportStep::Check, String::new());
        }
        self.sync_check();
    }

    fn answer_check(&mut self, a: Answer) {
        let measured = self.stats.measured_hz();
        let Some(v) = self.wizard.as_mut().and_then(|w| w.verify.as_mut()) else {
            return;
        };
        v.answer(a, measured, &mut self.mouse);
        if v.current().is_some() {
            self.sync_check();
            return;
        }
        self.finish_checks();
    }

    /// Cancel, Esc, close or hide: every changed setting is put back.
    fn cancel_checks(&mut self) {
        let Some(v) = self.wizard.as_mut().and_then(|w| w.verify.as_mut()) else {
            return;
        };
        v.cancel(&mut self.mouse);
        self.finish_checks();
    }

    /// Results go into the report.
    fn finish_checks(&mut self) {
        let Some(w) = self.wizard.as_mut() else {
            return;
        };
        let Some(v) = w.verify.take() else {
            return;
        };
        w.checks = v.lines().to_vec();
        let s = v.start();
        self.restore = Some(Restore { dpi: s.dpi, rate: s.rate, tries: RESTORE_TRIES });
        self.render_report();
        self.report_note(tr(T::ChecksDone));
    }

    /// Re-sends what the checks changed until the mouse reports it back.
    fn check_restore(&mut self, dpi: Option<u32>, rate: Option<u32>) {
        let Some(r) = self.restore.as_mut() else {
            return;
        };
        let cmds = restore_cmds(r, dpi, rate);
        r.tries = r.tries.saturating_sub(1);
        if cmds.is_empty() || r.tries == 0 {
            self.restore = None;
        }
        for c in cmds {
            self.mouse.send(c);
        }
    }

    fn close_report(&mut self) {
        self.cancel_checks();
        self.wizard = None;
        self.view.report = None;
    }

    /// The open check as shown; the rate check's measured value is live.
    fn sync_check(&mut self) {
        let measured = self.stats.measured_hz();
        let Some(v) = self.wizard.as_ref().and_then(|w| w.verify.as_ref()) else {
            return;
        };
        let Some(c) = v.current() else {
            return;
        };
        let check = check_view(v, c, measured);
        if let Some(r) = self.view.report.as_mut() {
            r.check = Some(check);
        }
        window::invalidate();
    }

    // ---- buttons ----

    /// Applies a new layout; refused if no button would stay a left click.
    fn set_buttons(&mut self, buttons: [Action; protocol::BUTTON_COUNT]) {
        let pending = self.undo.is_some();
        if store_layout(&mut self.cfg, &mut self.undo, buttons) == Layout::Refused {
            self.view.notice = tr(T::NeedLeft).into();
            return;
        }
        if pending {
            self.hide_toast();
        }
        self.view.notice.clear();
        self.view.buttons = buttons;
        self.send_setting(Command::SetButtons(self.cfg.buttons, self.cfg.macros.clone()));
        store::save_config(&self.cfg);
        self.sync_profiles();
    }

    fn assign(&mut self, action: Action) {
        let Some(p) = self.view.picker.take() else {
            return;
        };
        let mut b = self.cfg.buttons;
        b[p.button] = action;
        self.set_buttons(b);
    }

    /// "Reset all buttons": done at once, undoable from the toast.
    fn reset_buttons(&mut self) {
        let Some(u) = reset_layout(&mut self.cfg) else {
            return;
        };
        self.set_buttons(self.cfg.buttons);
        self.show_toast(tr(T::ToastReset).into(), Some(u));
    }

    fn clear_selected_macro(&mut self) {
        self.view.recording = false;
        let sel = self.view.macro_sel;
        let Some(u) = clear_macro(&mut self.cfg, sel) else {
            return;
        };
        self.macros_changed();
        self.show_toast(fill(tr(T::ToastCleared), &[&(sel + 1).to_string()]), Some(u));
    }

    /// Macros tab: plays the selected macro on button `b`; keeps the
    /// playback mode if the button already plays it.
    fn assign_macro(&mut self, b: usize) {
        self.view.dialog = None;
        let sel = self.view.macro_sel;
        let before = self.cfg.buttons;
        let Some(&current) = before.get(b) else {
            return;
        };
        let mode = match current {
            Action::Macro { index, mode } if index == sel => mode,
            _ => protocol::MacroMode::Times(1),
        };
        let mut next = before;
        next[b] = Action::Macro { index: sel, mode };
        self.set_buttons(next);

        // Refused (e.g. the last left click): the notice is on the Buttons tab.
        if self.cfg.buttons != next {
            self.view.page = Page::Buttons;
            return;
        }
        let name = fill(tr(T::ActMacro), &[&(sel + 1).to_string()]);
        let text = format!("{name} → {}", view::button_name(b));
        self.show_toast(text, Some(Undo::Buttons(before)));
    }

    /// Reverts the last destructive action while its toast shows.
    fn undo(&mut self) -> bool {
        let cover = if self.modal() { Cover::Modal } else { Cover::Free };
        let Some(resend) = ctrl_z(&mut self.cfg, &mut self.undo, cover) else {
            return false;
        };
        match resend {
            Resend::Buttons => self.set_buttons(self.cfg.buttons),
            Resend::Macros => self.macros_changed(),
            Resend::Layout => self.layout_changed(),
        }
        self.hide_toast();
        true
    }

    /// A later edit makes the pending undo stale: drop it with its toast.
    fn drop_undo(&mut self) {
        if self.undo.is_some() {
            self.hide_toast();
        }
    }

    fn show_toast(&mut self, text: String, undo: Option<Undo>) {
        self.view.toast = Some(Toast { text, undo: undo.is_some() });
        self.undo = undo;
        window::set_timer(TIMER_TOAST, TOAST_MS);
        window::invalidate();
    }

    fn hide_toast(&mut self) {
        window::kill_timer(TIMER_TOAST);
        self.view.toast = None;
        self.undo = None;
        window::invalidate();
    }

    /// Puts the command installing the udev rule on the clipboard.
    fn copy_command(&mut self) {
        let Some(cmd) = store::data_dir().and_then(|d| hid::grant_command(&d)) else {
            return;
        };
        if window::set_clipboard(&cmd) {
            self.show_toast(tr(T::Copied).into(), None);
        }
    }

    /// Puts the Mouse tab's facts on the clipboard for a bug report.
    fn copy_diagnostics(&mut self) {
        // English for public issues, whatever the UI language.
        let text = i18n::with_lang(Lang::En, || {
            let d = self.device_view();
            support_text(&d.rows, &d.status, &platform::os_version())
        });
        if window::set_clipboard(&text) {
            self.show_toast(tr(T::Copied).into(), None);
        }
    }

    // ---- sync chip ----

    /// A setting goes to the mouse: "Waiting for mouse…" until it answers.
    fn send_setting(&mut self, cmd: Command) {
        self.mouse.send(cmd);
        self.view.sync = Sync::Waiting;
        self.saved_at = None;
    }

    /// The mouse answered after a change: "Saved to mouse", then fade.
    fn mark_saved(&mut self) {
        if self.view.sync != Sync::Waiting {
            return;
        }
        self.saved_at = Some(Instant::now());
        self.view.sync = Sync::Saved(u8::MAX);
        window::set_timer(TIMER_FADE, FADE_TICK_MS);
    }

    /// Recording dot breathes while recording; the timer stops with it.
    fn pulse(&mut self) {
        if !self.view.recording {
            window::kill_timer(TIMER_PULSE);
            return;
        }
        self.view.pulse = pulse_alpha(self.rec_start.elapsed());
        window::invalidate();
    }

    fn fade_sync(&mut self) {
        let alpha = self.saved_at.and_then(|t| fade_alpha(t.elapsed()));
        match alpha {
            Some(a) if matches!(self.view.sync, Sync::Saved(_)) => self.view.sync = Sync::Saved(a),
            _ => {
                window::kill_timer(TIMER_FADE);
                if matches!(self.view.sync, Sync::Saved(_)) {
                    self.view.sync = Sync::Hidden;
                }
            }
        }
        window::invalidate();
    }

    fn edit_picker(&mut self, a: Act) {
        let Some(p) = self.view.picker.as_mut() else {
            return;
        };
        match a {
            Act::PickTab(t) => {
                p.tab = t;
                p.capturing = false;
                p.chosen = None;
            }
            Act::PickAction(action) => p.chosen = Some(action),
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

    /// Keyboard: shortcut capture in the picker, macro recording, then
    /// navigation (Tab, Enter, Esc, Ctrl+1..6, Ctrl+, Ctrl+Z).
    fn on_key(&mut self, k: KeyEvent) -> bool {
        if let Some(p) = self.view.picker.as_mut().filter(|p| p.capturing) {
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

        if self.view.recording {
            const VK_F4: u16 = 0x73;
            if k.vk == VK_F4 && window::modifiers().alt {
                self.stop_recording();
                return false;
            }
            if !k.repeat {
                self.record_key(k);
            }
            return true;
        }

        if !k.down {
            return false;
        }
        let used = self.on_key_down(k.vk, window::modifiers());
        if used {
            window::invalidate();
        }
        used
    }

    fn on_key_down(&mut self, vk: u16, m: Mods) -> bool {
        const VK_F10: u16 = 0x79;
        const VK_DIGIT_1: u16 = 0x31;
        const VK_DIGIT_9: u16 = 0x39;
        let bare = !(m.ctrl || m.alt || m.shift || m.win);
        // A profile name being typed takes every key.
        if self.rename_key(vk, m) {
            return true;
        }
        // F10 switches between the graphical and the text mode.
        if vk == VK_F10 && bare && !self.modal() {
            self.on_act(Act::ToggleTui);
            return true;
        }
        // Text mode: the digits are the menu.
        if self.view.tui && bare && !self.modal() && (VK_DIGIT_1..=VK_DIGIT_9).contains(&vk) {
            if let Some(page) = view::page_of_key(&self.view, (vk - VK_DIGIT_1) as usize + 1) {
                self.on_act(Act::Nav(page));
                return true;
            }
        }
        // The report's pick list and text take the arrow keys.
        if let Some(r) = self.view.report.as_mut() {
            if matches!(vk, VK_UP | VK_DOWN | VK_PRIOR | VK_NEXT) {
                return report_key(r, vk);
            }
        }
        // The detect dialog's lists take the arrow keys and Tab.
        if let Some(d) = self.view.detect.as_mut().filter(|_| !self.view.ask_setup) {
            if matches!(vk, VK_UP | VK_DOWN | VK_LEFT | VK_RIGHT | VK_TAB) && detect_key(d, vk) {
                return true;
            }
        }
        let cover = if self.modal() { Cover::Modal } else { Cover::Free };
        match key_action(vk, m, self.view.focus, &self.acts, &self.view.caps, cover) {
            Key::Close => self.close_top(),
            Key::Focus(step) => {
                self.view.focus = next_focus(&self.acts, self.view.focus, step);
                true
            }
            Key::Press(a) => {
                self.on_act(a);
                true
            }
            Key::Default => self.press_default(),
            Key::Nudge(step) => self.nudge(step),
            Key::Undo => self.undo(),
            Key::Page(p) => {
                self.on_act(Act::Nav(p));
                true
            }
            Key::Ignore => false,
        }
    }

    /// A dialog or picker covers the page.
    fn modal(&self) -> bool {
        self.view.ask_setup || self.view.detect.is_some() || self.view.picker.is_some() || self.view.dialog.is_some() || self.view.report.is_some()
    }

    /// Esc: closes the topmost dialog without side effects.
    fn close_top(&mut self) -> bool {
        if self.view.ask_setup {
            return false;
        }
        // An open list closes before anything under it.
        if std::mem::take(&mut self.view.lang_menu) {
            return true;
        }
        let checking = self.wizard.as_ref().is_some_and(|w| w.verify.is_some());
        let closed = if checking {
            // Esc during the checks ends them; the report and its results stay.
            self.cancel_checks();
            true
        } else if self.view.report.is_some() {
            // On top of the detect dialog when opened from it.
            self.close_report();
            true
        } else if let Some(d) = self.view.detect.as_mut() {
            // Step 2 goes back to the detected mice first.
            if !step_back(d) {
                self.cancel_detect();
            }
            true
        } else {
            self.view.picker.take().is_some() || self.view.dialog.take().is_some()
        };
        // Whatever closed, its focused element went with it.
        self.view.focus.take().is_some() || closed
    }

    /// Enter with nothing focused: the open dialog's main button.
    fn press_default(&mut self) -> bool {
        let step = self.view.report.as_ref().map(|r| r.step);
        let act = if step == Some(ReportStep::Pick) {
            Act::ReportRead
        } else if step.is_some() {
            return false;
        } else if self.view.detect.is_some() && !self.view.ask_setup {
            Act::DetectUse
        } else if self.view.picker.is_some_and(|p| p.built().is_some()) {
            Act::PickApply
        } else {
            return false;
        };
        self.on_act(act);
        true
    }

    /// Left / Right on a focused slider: one step.
    fn nudge(&mut self, step: Step) -> bool {
        let delta = |i: usize, n: usize| match step {
            Step::Next => (i + 1).min(n.saturating_sub(1)),
            Step::Prev => i.saturating_sub(1),
        };
        match self.view.focus.and_then(|x| x.locate(&self.acts)).map(|i| self.acts[i]) {
            Some(Act::Slider(i)) => {
                let Some(s) = self.view.stages.get(i) else {
                    return false;
                };
                self.set_stage(i, delta(s.index, self.view.steps));
                true
            }
            Some(Act::BasicDpi) => {
                let vals = &self.view.basic.values;
                let cur = self.view.basic.dpi.and_then(|d| vals.iter().position(|&x| x >= d)).unwrap_or(0);
                self.drag_basic_dpi(delta(cur, vals.len()), Phase::Done);
                true
            }
            _ => false,
        }
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
        self.drop_undo();
        self.view.macros = self.cfg.macros.clone();
        store::save_config(&self.cfg);
        self.sync_profiles();
        if self.cfg.buttons.iter().any(|a| matches!(a, Action::Macro { .. })) {
            self.send_setting(Command::SetButtons(self.cfg.buttons, self.cfg.macros.clone()));
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
        self.send_setting(Command::SetLighting(self.cfg.lighting.clone()));
        store::save_config(&self.cfg);
    }

    fn push_power(&mut self) {
        self.view.power = self.cfg.power;
        self.send_setting(Command::SetPower(self.cfg.power));
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
        self.send_setting(Command::SetDpiColors(self.cfg.dpi_colors));
        store::save_config(&self.cfg);
    }

    // ---- polling ----

    fn set_polling(&mut self, i: usize) {
        let Some(&rate) = PollingRate::ALL.get(i) else {
            return;
        };
        self.cfg.polling = rate;
        self.view.polling = i;
        self.send_setting(Command::SetPolling(rate));
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
                self.view.page = Page::General;
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
        let device = self.cfg.device.as_deref();
        let other = device.is_some() && device.and_then(devices::by_id).is_none();
        self.view.device_name = if other { other_name(self.chosen_detected()) } else { device_name(device) };
        if let Some(i) = self.ident.clone() {
            self.name_from(&i);
        }
        let facts = self.facts();
        relabel(&mut self.view, self.link, &facts);
        self.update_tray();
        self.sync_profiles();
        self.refresh_stats();
    }

    fn uninstall(&mut self) {
        if install::uninstall(platform::Origin::InApp) {
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
        self.view.measured = self.stats.measured_hz();
        // Keep the last value: the card must not blink when the mouse rests.
        self.view.last_rate = self.view.measured.or(self.view.last_rate);
        self.sync_check();

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
        (self.view.chart, self.view.chart_ticks, self.view.chart_since) = chart(&history, stats::unix_now(), self.period);
        self.view.battery_note = battery_note(stats::hours_left(&history));
        window::invalidate();
    }
}

/// Debug builds: `WHISKER_DEMO=1` turns "no receiver" / "asleep" into a
/// connected mouse at 78 %, for screenshots without the mouse at hand.
#[cfg(debug_assertions)]
fn demo_status(ev: Event, active: usize) -> Event {
    const DEMO_VAR: &str = "WHISKER_DEMO";
    const DEMO_BATTERY: u8 = 78;
    if std::env::var_os(DEMO_VAR).is_none() || !matches!(ev, Event::Absent | Event::Offline) {
        return ev;
    }
    Event::Status(Status {
        battery: DEMO_BATTERY,
        charge: protocol::Charge::Discharging,
        active_stage: active,
        sensor: protocol::sensor().id,
        raw: [0; protocol::REPORT_LEN],
    })
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
/// Battery samples of the period as (0..1 time, %) points, plus X labels:
/// hours back for today (last 24 h), dates for 7 days and all history.
/// A history shorter than the period starts the axis at its first sample
/// and returns that day ("collecting since").
fn chart(samples: &[stats::BatterySample], now: u64, period: Period) -> (Vec<(f32, f32)>, Vec<(f32, String)>, Option<String>) {
    const HOUR_TICKS: [u64; 3] = [18, 12, 6];
    const HOUR_SECS: u64 = 3600;
    let days = match period {
        Period::Today => 1,
        Period::Week => CHART_DAYS,
        // Whole days back to the oldest sample.
        Period::All => samples.first().map_or(1, |s| now.saturating_sub(s.t).div_ceil(DAY_SECS).max(1)),
    };
    let period_start = now.saturating_sub(days * DAY_SECS);
    let first = samples.first().map(|s| s.t).filter(|&t| t > period_start && t < now);
    let start = first.unwrap_or(period_start);
    let span = (now - start).max(1);
    let at = |back: u64| 1.0 - back as f32 / span as f32;
    let points = samples.iter().filter(|s| s.t >= start).map(|s| ((s.t - start) as f32 / span as f32, s.pct as f32)).collect();

    let ticks = match period {
        Period::Today => {
            let hours = HOUR_TICKS.iter().filter(|h| *h * HOUR_SECS <= span);
            let mut t: Vec<(f32, String)> = hours.map(|h| (at(h * HOUR_SECS), format!("−{h} h"))).collect();
            t.push((1.0, tr(T::ChartNow).into()));
            t
        }
        _ => {
            let step = days.div_ceil(CHART_MAX_TICKS);
            let within = (0..=days).step_by(step as usize).filter(|d| d * DAY_SECS <= span);
            within.map(|d| (at(d * DAY_SECS), day_label(d as i64))).collect()
        }
    };
    let since = first.map(|t| day_label(((now - t) / DAY_SECS) as i64));
    (points, ticks, since)
}

/// Date `days_back` days ago, e.g. "11/9" (English, East Asian) or "9.11.".
fn day_label(days_back: i64) -> String {
    let (m, d) = stats::month_day(days_back);
    match i18n::lang() {
        Lang::En | Lang::Ja | Lang::Ko | Lang::ZhHans | Lang::ZhHant => format!("{m}/{d}"),
        _ => format!("{d}.{m}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coarse_battery_is_approximate() {
        // M705 reports 50 % as one of four steps: no warning colour.
        let mut v = initial_view(&AppConfig::default(), false);
        (v.connected, v.has_battery, v.battery, v.battery_approx) = (true, true, 50, true);
        assert_eq!(view::battery_text(&v), "≈ 50 %");
        assert!(view::battery_color(&v) == th_good());

        // An exact 50 % still warns; low stays red either way.
        v.battery_approx = false;
        assert_eq!(view::battery_text(&v), "50 %");
        assert!(view::battery_color(&v) != th_good());
        (v.battery, v.battery_approx) = (5, true);
        assert!(view::battery_color(&v) != th_good());
    }

    #[test]
    fn coarse_battery_has_no_life_estimate() {
        // Four battery steps give no usable runtime estimate.
        let mut v = initial_view(&AppConfig::default(), false);
        (v.connected, v.has_battery, v.battery, v.charge) = (true, true, 50, Charge::Discharging);
        v.battery_note = battery_note(None);
        assert_eq!(view::battery_hint(&v), tr(T::EtaPending));

        v.battery_approx = true;
        assert_eq!(view::battery_hint(&v), "");
        v.battery_note = battery_note(Some(100.0));
        assert_eq!(view::battery_hint(&v), "");
    }

    fn th_good() -> crate::gfx::Color {
        crate::theme::current().good
    }

    #[test]
    fn switching_to_driverless_mouse_drops_old_battery() {
        // Lionel was connected at 56 %, then "other mouse" is chosen.
        let mut v = initial_view(&AppConfig::default(), false);
        (v.connected, v.has_battery, v.battery) = (true, true, 56);
        v.access = Access::Denied;
        forget_mouse(&mut v, CAPS_NONE);
        assert!(!v.connected);
        // The permission hint belonged to the old mouse.
        assert_eq!(v.access, Access::Granted);
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

    fn lionel() -> DeviceInfo {
        let m = devices::default_model();
        DeviceInfo { vid: m.vid, pid: m.pids[0].0, product: "Lionel".into(), ..DeviceInfo::default() }
    }

    #[test]
    fn detect_cancel_keeps_mouse_or_settles_first_run() {
        // Later: Cancel leaves the chosen mouse alone.
        assert_eq!(cancel_choice(DetectStep::Found, Some("whiteshark/lionel")), None);
        // First run: Cancel means "other mouse", so the question isn't repeated.
        assert_eq!(cancel_choice(DetectStep::Found, None), Some(store::GENERIC_DEVICE));
        // Still looking for mice: nothing was asked, the list still has to.
        assert_eq!(cancel_choice(DetectStep::Searching, None), None);
    }

    /// A catalogue model's device as the OS would list it.
    fn device_of(id: &str) -> DeviceInfo {
        let m = devices::by_id(id).unwrap();
        DeviceInfo { vid: m.vid, pid: m.pids[0].0, ..DeviceInfo::default() }
    }

    #[test]
    fn detect_preselects_the_current_mouse() {
        let mice = [lionel(), device_of("corsair/katar-pro")];
        let d = detect_state(&mice, Some("corsair/katar-pro"), None, "");
        let (b, k) = locate("corsair/katar-pro").unwrap();
        assert_eq!((d.brand, d.model, d.chip), (b, k, Some(1)));
        assert!(d.found[1].current && !d.found[0].current);
        assert_eq!(d.models[d.model].choice, "corsair/katar-pro");

        // Old id in the config: same place.
        let d = detect_state(&mice, Some("whiteshark-lionel"), None, "");
        assert_eq!((d.brand, d.model, d.chip), (0, 0, Some(0)));

        // First run: the first detected mouse Whisker can drive.
        let unknown = DeviceInfo { vid: 0x1234, pid: 1, ..DeviceInfo::default() };
        let d = detect_state(&[unknown, device_of("corsair/katar-pro")], None, None, "");
        assert_eq!((d.brand, d.model, d.chip), (b, k, Some(1)));
        // A current mouse that is not connected is still selected.
        let d = detect_state(&[], Some("roccat/auto"), None, "");
        assert_eq!(d.models[d.model].choice, "roccat/auto");
        assert_eq!(d.chip, None);
    }

    #[test]
    fn detect_lists_scroll_with_the_selection() {
        let mut d = detect_state(&[], Some("steelseries/auto"), None, "");
        assert_eq!(d.step, DetectStep::List);
        assert!(d.models.len() > view::LIST_ROWS, "SteelSeries lists more models than fit");
        assert!(d.model >= d.model_top && d.model < d.model_top + view::LIST_ROWS);
        detect_wheel(&mut d, Pane::Models, 100);
        assert_eq!(d.model_top, 0);
        for _ in 0..d.models.len() {
            detect_key(&mut d, VK_DOWN);
        }
        assert_eq!(d.model, d.models.len() - 1);
        assert_eq!(d.model_top, d.models.len() - view::LIST_ROWS);

        // Left: the brand list takes Up/Down; a new brand starts at its top.
        detect_key(&mut d, VK_LEFT);
        detect_key(&mut d, VK_UP);
        assert_eq!(d.pane, Pane::Brands);
        assert_eq!((d.model, d.model_top), (0, 0));
    }

    #[test]
    fn detect_use_keeps_the_picked_device() {
        let mice = [device_of("corsair/katar-pro")];
        let mut d = detect_state(&mice, None, None, "");
        let usb = (mice[0].vid, mice[0].pid);
        assert_eq!(detect_choice(&d), Some(("corsair/katar-pro".into(), Some(usb))));

        // Another model of the brand: no longer that device.
        show_list(&mut d);
        let other = d.models.iter().position(|m| m.choice == "corsair/auto").unwrap();
        pick_model(&mut d, other);
        assert_eq!(detect_choice(&d), Some(("corsair/auto".into(), None)));
    }

    #[test]
    fn detect_starts_on_the_detected_mice() {
        let d = detect_state(&[lionel()], None, None, "");
        assert_eq!(d.step, DetectStep::Found);

        // Nothing detected: the lists, with no step to go back to.
        let mut d = detect_state(&[], Some("corsair/katar-pro"), None, "");
        assert_eq!(d.step, DetectStep::List);
        assert!(!step_back(&mut d));
        assert_eq!(d.step, DetectStep::List);
    }

    #[test]
    fn detect_back_restores_the_detected_pick() {
        let mice = [lionel(), device_of("corsair/katar-pro")];
        let usb = (mice[1].vid, mice[1].pid);
        let mut d = detect_state(&mice, None, None, "");
        pick_found(&mut d, 1);
        show_list(&mut d);
        assert_eq!(d.step, DetectStep::List);

        // Browsing another brand leaves the detected mouse; Back returns to it.
        pick_brand(&mut d, 0);
        assert_eq!(d.chip, None);
        assert!(step_back(&mut d));
        assert_eq!((d.step, d.chip), (DetectStep::Found, Some(1)));
        assert_eq!(detect_choice(&d), Some(("corsair/katar-pro".into(), Some(usb))));
    }

    #[test]
    fn detect_names_the_identified_mouse() {
        const UNIFYING: u16 = 0xC52B;
        const M705: &str = "Logitech Marathon Mouse/Performance Plus M705";
        let auto = devices::by_id("logitech/auto").unwrap();
        let receiver = DeviceInfo { vid: auto.vid, pid: UNIFYING, product: "USB Receiver".into(), ..DeviceInfo::default() };

        // The driver named the mouse behind the receiver: its name and support.
        let d = detect_state(std::slice::from_ref(&receiver), Some("logitech/auto"), None, M705);
        let f = &d.found[0];
        assert_eq!(f.title, M705);
        assert_eq!((f.kind, f.via), (Badge::Good, Some(T::ViaReceiver)));
        assert_eq!(d.models[d.model].kind, Badge::Good, "step 2 agrees with step 1");

        // Not identified yet: what the receiver reports.
        let d = detect_state(&[receiver], Some("logitech/auto"), None, "");
        assert_eq!((d.found[0].title.as_str(), d.found[0].kind), ("Logitech USB Receiver", Badge::Warn));
        assert_eq!(d.found[0].via, None);
    }

    #[test]
    fn detect_use_needs_a_detected_pick() {
        // The current mouse is not connected: nothing picked on step 1 yet.
        let mut d = detect_state(&[lionel(), device_of("corsair/katar-pro")], Some("roccat/auto"), None, "");
        assert_eq!((d.step, d.chip), (DetectStep::Found, None));
        assert_eq!(detect_choice(&d), None);

        // Up / Down move among the detected mice; Tab is left to the focus.
        assert!(detect_key(&mut d, VK_DOWN));
        assert_eq!(d.chip, Some(0));
        assert!(detect_key(&mut d, VK_DOWN) && detect_key(&mut d, VK_DOWN));
        assert_eq!(d.chip, Some(1));
        assert!(!detect_key(&mut d, VK_TAB));
        assert!(!d.keys, "no list outline on step 1");
    }

    #[test]
    fn scan_keeps_the_detected_mouse_picked() {
        let mice = [lionel(), device_of("corsair/katar-pro")];
        let mut d = detect_state(&mice, Some("whiteshark/lionel"), None, "");
        pick_found(&mut d, 1);
        let (brand, model) = (d.brand, d.model);
        let katar = (mice[1].vid, mice[1].pid);

        // A scan reports the same mice, in another order.
        merge_detect(&mut d, &[mice[1].clone(), mice[0].clone()], Some("whiteshark/lionel"), None, "");
        assert_eq!((d.step, d.chip, d.brand, d.model), (DetectStep::Found, Some(0), brand, model));
        assert_eq!(detect_choice(&d), Some(("corsair/katar-pro".into(), Some(katar))));
    }

    #[test]
    fn scan_preselects_again_when_the_picked_mouse_is_gone() {
        let mice = [lionel(), device_of("corsair/katar-pro")];
        let mut d = detect_state(&mice, None, None, "");
        pick_found(&mut d, 1);

        merge_detect(&mut d, std::slice::from_ref(&mice[0]), None, None, "");
        assert_eq!((d.step, d.chip), (DetectStep::Found, Some(0)), "the mouse left is picked, as on opening");
        assert_eq!(detect_choice(&d), Some((devices::default_model().id.clone(), Some((mice[0].vid, mice[0].pid)))));
    }

    #[test]
    fn scan_keeps_the_browsing_of_the_lists() {
        let mut d = detect_state(&[lionel()], None, None, "");
        show_list(&mut d);
        pick_brand(&mut d, locate("steelseries/auto").unwrap().0);
        let last = d.models.len() - 1;
        pick_model(&mut d, last);
        assert!(detect_key(&mut d, VK_LEFT));
        let browsed = (d.step, d.pane, d.brand, d.model, d.model_top);
        assert_eq!(d.model_top, last + 1 - view::LIST_ROWS);

        merge_detect(&mut d, &[lionel(), device_of("corsair/katar-pro")], None, None, "");
        assert_eq!((d.step, d.pane, d.brand, d.model, d.model_top), browsed, "one mouse more changes nothing of the browsing");
        assert_eq!((d.chip, d.kept), (None, Some(0)), "the detected pick is remembered for Back");
        assert_eq!(d.found.len(), 2);
    }

    #[test]
    fn a_dialog_that_opened_with_nothing_takes_the_new_mice() {
        // A scan found no mouse: the dialog opened with the lists empty.
        let mut d = detect_state(&[], None, None, "");
        assert_eq!((d.step, d.chip), (DetectStep::List, None));

        merge_detect(&mut d, &[lionel()], None, None, "");
        assert_eq!((d.step, d.chip), (DetectStep::Found, Some(0)));
    }

    #[test]
    fn searching_dialog_waits_for_the_first_list() {
        let mut d = searching_state(Some("whiteshark/lionel"));
        assert_eq!((d.step, d.chip, d.found.is_empty(), d.refetching), (DetectStep::Searching, None, true, false));
        // Nothing to use, nothing to move, no step to go back to.
        assert_eq!(detect_choice(&d), None);
        assert!(!detect_key(&mut d, VK_DOWN) && !d.keys);
        assert!(!step_back(&mut d));
    }

    #[test]
    fn the_first_list_replaces_the_searching_dialog() {
        let mouse = lionel();
        let mut d = searching_state(None);
        merge_detect(&mut d, std::slice::from_ref(&mouse), None, None, "");
        assert_eq!((d.step, d.chip), (DetectStep::Found, Some(0)));
        assert_eq!(detect_choice(&d), Some((devices::default_model().id.clone(), Some((mouse.vid, mouse.pid)))));
    }

    #[test]
    fn the_answer_clears_the_refetching_note() {
        let mice = [lionel(), device_of("corsair/katar-pro")];
        let mut d = detect_state(&mice, None, None, "");
        show_list(&mut d);
        pick_brand(&mut d, locate("steelseries/auto").unwrap().0);
        let browsed = d.brand;
        d.refetching = true;

        // The very same list lands: the note goes, the browsing stays.
        merge_detect(&mut d, &mice, None, None, "");
        assert_eq!((d.step, d.brand, d.refetching), (DetectStep::List, browsed, false));
    }

    #[test]
    fn a_search_that_found_nothing_leaves_the_catalog_open() {
        let mut d = searching_state(None);
        merge_detect(&mut d, &[], None, None, "");
        assert_eq!((d.step, d.chip), (DetectStep::List, None), "search over: nothing detected, the lists are open");
    }

    #[test]
    fn chosen_device_is_what_the_driver_may_open() {
        // Picked from the detected mice: exactly that product.
        let pick = pick_for(Some("corsair/auto"), Some((0x1B1C, 0x1B80)));
        assert_eq!(pick, Pick { vid: Some(0x1B1C), pids: vec![0x1B80] });
        assert!(pick.allows(0x1B1C, 0x1B80) && !pick.allows(0x1B1C, 0x1B81));
        // Picked from the list: the model's products, or the whole vendor.
        let katar = devices::by_id("corsair/katar-pro").unwrap();
        assert_eq!(pick_for(Some("corsair/katar-pro"), None).pids, katar.pids.iter().map(|(p, _)| *p).collect::<Vec<_>>());
        assert_eq!(pick_for(Some("corsair/auto"), None), Pick { vid: Some(0x1B1C), pids: Vec::new() });
        // Other mouse: no driver, nothing to limit.
        assert_eq!(pick_for(Some(store::GENERIC_DEVICE), None), Pick::default());
    }

    #[test]
    fn undo_restores_reset_buttons() {
        let mut cfg = AppConfig::default();
        assert_eq!(reset_layout(&mut cfg), None, "already default: nothing to undo");

        cfg.buttons[2] = Action::Disable;
        let custom = cfg.buttons;
        let u = reset_layout(&mut cfg).unwrap();
        assert_eq!(cfg.buttons, DEFAULT_ACTIONS);
        assert_eq!(apply_undo(&mut cfg, u), Resend::Buttons);
        assert_eq!(cfg.buttons, custom);
    }

    #[test]
    fn undo_restores_cleared_macro() {
        let mut cfg = AppConfig::default();
        assert_eq!(clear_macro(&mut cfg, 1), None, "empty macro: Clear is disabled");

        let steps = vec![MacroEvent { ev: Ev::Key(0x04, Edge::Down), delay_ms: 20 }, MacroEvent { ev: Ev::Key(0x04, Edge::Up), delay_ms: 0 }];
        cfg.macros[1] = steps.clone();
        let u = clear_macro(&mut cfg, 1).unwrap();
        assert!(cfg.macros[1].is_empty());
        assert_eq!(apply_undo(&mut cfg, u), Resend::Macros);
        assert_eq!(cfg.macros[1], steps);
    }

    #[test]
    fn caps_follow_what_the_mouse_reports() {
        // Logitech M705: battery only, fixed DPI and report rate.
        let basic = devices::by_id("logitech/auto").expect("catalogue").caps;
        let m705 = Ability { battery: true, ..Ability::default() };
        let caps = narrow_caps(basic, &m705);
        assert_eq!(caps.dpi, DpiUi::None);
        assert!(!caps.polling);
        assert_eq!(view::nav_pages(&caps), [Page::Overview, Page::Stats]);

        // A mouse that reports both keeps its pages.
        let g502 = Ability { dpi: Some(crate::drivers::DpiRange::Steps { min: 100, max: 25600, step: 50 }), rates: vec![125, 1000], battery: false, stages: None };
        assert_eq!(narrow_caps(basic, &g502), basic);

        // Lionel-style stage mice are not narrowed.
        let lionel = devices::default_model().caps;
        assert_eq!(narrow_caps(lionel, &Ability::default()), lionel);
    }

    #[test]
    fn nav_shows_only_supported_pages() {
        let all = view::nav_pages(&devices::default_model().caps);
        assert_eq!(all, [Page::Overview, Page::Dpi, Page::Polling, Page::Lighting, Page::Buttons, Page::Profiles, Page::Stats]);
        assert_eq!(view::nav_pages(&CAPS_NONE), [Page::Overview, Page::Stats]);

        // Tabs follow their sidebar page; Settings is always there.
        assert!(!view::page_visible(Page::Macros, &CAPS_NONE));
        // Profiles hold button layouts: nothing for a mouse without them.
        assert!(!view::page_visible(Page::Profiles, &CAPS_NONE));
        assert!(view::page_visible(Page::Mouse, &CAPS_NONE));
        assert_eq!(Page::Appearance.nav(), Page::General);
    }

    #[test]
    fn ctrl_digits_switch_visible_pages() {
        let ctrl = Mods { ctrl: true, ..Mods::default() };
        let caps = devices::default_model().caps;
        assert_eq!(page_shortcut(VK_1, ctrl, &caps), Some(Page::Overview));
        assert_eq!(page_shortcut(VK_1 + 5, ctrl, &caps), Some(Page::Profiles));
        assert_eq!(page_shortcut(VK_1 + 6, ctrl, &caps), Some(Page::Stats));
        assert_eq!(page_shortcut(VK_COMMA, ctrl, &caps), Some(Page::General));
        // Without Ctrl a digit is just a digit.
        assert_eq!(page_shortcut(VK_1, Mods::default(), &caps), None);

        // Statistics only: Ctrl+2 is the second visible page.
        assert_eq!(page_shortcut(VK_1 + 1, ctrl, &CAPS_NONE), Some(Page::Stats));
        assert_eq!(page_shortcut(VK_1 + 2, ctrl, &CAPS_NONE), None);
        assert_eq!(page_shortcut(VK_Z, ctrl, &caps), None);
    }

    #[test]
    fn tab_focus_wraps() {
        let acts = [Act::Nav(Page::Overview), Act::About, Act::Undo];
        let at = |i: usize| Some(Focus { index: i, act: acts[i] });
        assert_eq!(next_focus(&acts, None, Step::Next), at(0));
        assert_eq!(next_focus(&acts, None, Step::Prev), at(2));
        assert_eq!(next_focus(&acts, at(2), Step::Next), at(0));
        assert_eq!(next_focus(&acts, at(0), Step::Prev), at(2));
        assert_eq!(next_focus(&[], None, Step::Next), None);
    }

    #[test]
    fn tab_visits_every_hit_once_despite_repeated_acts() {
        // Settings › General in drawing order: About (logo) and Mouse (name)
        // repeat further down as the About item and the Mouse tab.
        let mut acts = vec![Act::About, Act::Nav(Page::Mouse)];
        acts.extend(view::nav_pages(&devices::default_model().caps).into_iter().map(Act::Nav));
        acts.extend([Act::About, Act::Nav(Page::General)]);
        acts.extend([Act::Nav(Page::General), Act::Nav(Page::Appearance), Act::Nav(Page::Mouse), Act::Install, Act::Autostart]);

        let mut seen = Vec::new();
        let mut f = None;
        for _ in 0..acts.len() {
            f = next_focus(&acts, f, Step::Next);
            seen.push(f.unwrap().index);
        }
        assert_eq!(seen, (0..acts.len()).collect::<Vec<_>>());
        assert!(seen.iter().any(|&i| acts[i] == Act::Autostart));

        // Shift+Tab walks back over the same positions.
        let back = next_focus(&acts, f, Step::Prev).unwrap();
        assert_eq!(back.index, acts.len() - 2);
    }

    #[test]
    fn recording_dot_pulses_once_a_second() {
        assert_eq!(pulse_alpha(Duration::ZERO), u8::MAX);
        assert!(pulse_alpha(PULSE_PERIOD / 2) < 100);
        assert_eq!(pulse_alpha(PULSE_PERIOD), u8::MAX);
    }

    #[test]
    fn saved_chip_fades_out() {
        assert_eq!(fade_alpha(Duration::ZERO), Some(u8::MAX));
        let mid = fade_alpha(SAVED_HOLD + SAVED_FADE / 2).unwrap();
        assert!(mid > 100 && mid < 150);
        assert_eq!(fade_alpha(SAVED_HOLD + SAVED_FADE), None);
    }

    /// A catalogue mouse as the OS would list it.
    fn listed(id: &str, serial: &str) -> DeviceInfo {
        let m = devices::by_id(id).unwrap();
        // Vendor-wide entries take any product id.
        let pid = m.pids.first().map_or(0x0001, |p| p.0);
        DeviceInfo { vid: m.vid, pid, serial: serial.into(), ..DeviceInfo::default() }
    }

    const LIONEL: &str = "whiteshark/lionel";
    const ETERNICO: &str = "eternico/aet-ms480b";

    #[test]
    fn banner_offers_a_new_known_mouse() {
        let (lionel, eternico) = (listed(LIONEL, ""), listed(ETERNICO, ""));
        let seen = vec![mouse_key(&lionel)];
        let mice = [lionel, eternico.clone()];
        assert_eq!(new_mouse(&mice, &seen, Some(LIONEL), &[]), Some(&eternico));
        let b = banner_of(&eternico).unwrap();
        assert_eq!(b.name, devices::by_id(ETERNICO).unwrap().full_name());

        // Statistics-only models are not worth a banner.
        let basic = listed("eternico/aet-mvs490b", "");
        assert_eq!(new_mouse(&[basic], &[], Some(LIONEL), &[]), None);

        // Already there at the last scan: no news.
        let seen: Vec<String> = mice.iter().map(mouse_key).collect();
        assert_eq!(new_mouse(&mice, &seen, Some(LIONEL), &[]), None);
        // Before the first-run choice the detect dialog asks instead.
        assert_eq!(new_mouse(&mice, &[], None, &[]), None);
    }

    /// A config that chose `current` and remembers `others` too, `current` last used.
    fn remembering(current: &DeviceInfo, others: &[&DeviceInfo]) -> AppConfig {
        let mut cfg = AppConfig::default();
        let mut undo = None;
        for d in others.iter().copied().chain([current]) {
            switch_device(&mut cfg, &mut undo, offer_choice(d), Some((d.vid, d.pid)));
        }
        cfg
    }

    #[test]
    fn a_remembered_mouse_is_switched_to_when_it_is_connected() {
        let (lionel, eternico) = (listed(LIONEL, ""), listed(ETERNICO, ""));
        let cfg = remembering(&lionel, &[&eternico]);
        let both = [lionel.clone(), eternico.clone()];
        let seen = vec![mouse_key(&lionel)];
        let k = remembered(&both, Some(&seen), &cfg).expect("the remembered mouse was just plugged in");
        assert_eq!((k.device.as_str(), k.usb), (ETERNICO, (eternico.vid, eternico.pid)));

        // Both were there at the last scan, or at start: the choice stays.
        let seen: Vec<String> = both.iter().map(mouse_key).collect();
        assert_eq!(remembered(&both, Some(&seen), &cfg), None);
        assert_eq!(remembered(&both, None, &cfg), None);
    }

    #[test]
    fn a_missing_mouse_gives_way_to_a_remembered_one() {
        let (lionel, eternico) = (listed(LIONEL, ""), listed(ETERNICO, ""));
        let cfg = remembering(&lionel, &[&eternico]);
        let only = [eternico.clone()];
        let seen = vec![mouse_key(&eternico)];
        // At start and after the chosen one was unplugged.
        assert_eq!(remembered(&only, None, &cfg).map(|k| k.device.as_str()), Some(ETERNICO));
        assert_eq!(remembered(&only, Some(&seen), &cfg).map(|k| k.device.as_str()), Some(ETERNICO));
        // Nothing remembered is connected: nothing to switch to.
        assert_eq!(remembered(&[], Some(&seen), &cfg), None);
    }

    #[test]
    fn only_chosen_mice_are_switched_to() {
        let (lionel, eternico) = (listed(LIONEL, ""), listed(ETERNICO, ""));
        // Never chosen: the banner asks instead.
        let cfg = remembering(&lionel, &[]);
        assert_eq!(remembered(&[lionel.clone(), eternico.clone()], Some(&[]), &cfg), None);

        // "Not now" forgets a mouse; choosing it again ends the "Not now".
        let mut cfg = remembering(&lionel, &[&eternico]);
        ignore_mouse(&mut cfg, &eternico);
        assert_eq!(remembered(std::slice::from_ref(&eternico), None, &cfg), None);
        switch_device(&mut cfg, &mut None, ETERNICO.into(), Some((eternico.vid, eternico.pid)));
        assert_eq!(cfg.ignored_mice, Vec::<String>::new());

        // Before the first choice the detect dialog asks.
        let mut fresh = AppConfig::default();
        fresh.remember_mouse(ETERNICO, (eternico.vid, eternico.pid));
        assert_eq!(remembered(&[eternico], None, &fresh), None);
    }

    #[test]
    fn a_choice_from_the_lists_counts_by_its_model() {
        let (lionel, eternico) = (listed(LIONEL, ""), listed(ETERNICO, ""));
        let mut cfg = remembering(&lionel, &[&eternico]);
        // Chosen from the catalogue, without USB ids.
        switch_device(&mut cfg, &mut None, LIONEL.into(), None);
        let seen: Vec<String> = [&lionel, &eternico].into_iter().map(mouse_key).collect();
        assert_eq!(remembered(&[lionel, eternico.clone()], Some(&seen), &cfg), None);
        assert_eq!(remembered(&[eternico], Some(&seen), &cfg).map(|k| k.device.as_str()), Some(ETERNICO));
    }

    #[test]
    fn banner_skips_the_current_and_unknown_mice() {
        let cable = DeviceInfo { pid: devices::by_id(LIONEL).unwrap().pids[1].0, ..listed(LIONEL, "") };
        assert_eq!(new_mouse(&[cable], &[], Some(LIONEL), &[]), None);

        let unknown = DeviceInfo { vid: 0x1234, pid: 0x5678, product: "Office mouse".into(), ..DeviceInfo::default() };
        assert_eq!(new_mouse(std::slice::from_ref(&unknown), &[], Some(LIONEL), &[]), None);
        assert_eq!(new_mouse(&[unknown], &[], Some(store::GENERIC_DEVICE), &[]), None);

        // A receiver the chosen model pairs through is that mouse.
        let bolt = DeviceInfo { vid: 0x046D, pid: 0xC548, ..DeviceInfo::default() };
        assert_eq!(new_mouse(std::slice::from_ref(&bolt), &[], Some("logitech/mx-master-3"), &[]), None);
        assert_eq!(new_mouse(std::slice::from_ref(&bolt), &[], Some(LIONEL), &[]), Some(&bolt));
    }

    #[test]
    fn not_now_is_remembered_per_mouse() {
        let mut cfg = AppConfig { device: Some(LIONEL.into()), ..AppConfig::default() };
        let first = listed(ETERNICO, "A1-B2");
        ignore_mouse(&mut cfg, &first);
        ignore_mouse(&mut cfg, &first);
        assert_eq!(cfg.ignored_mice, ["1D57:FA60:A1B2"]);
        assert_eq!(new_mouse(std::slice::from_ref(&first), &[], Some(LIONEL), &cfg.ignored_mice), None);

        // Another one of the same model, with its own serial, still asks.
        let second = listed(ETERNICO, "C3");
        assert_eq!(new_mouse(std::slice::from_ref(&second), &[], Some(LIONEL), &cfg.ignored_mice), Some(&second));
    }

    #[test]
    fn switch_retargets_to_the_offered_mouse() {
        let mut cfg = AppConfig { device: Some(LIONEL.into()), ..AppConfig::default() };
        let mut undo = Some(Undo::Buttons(DEFAULT_ACTIONS));
        let offered = listed("razer/auto", "");
        switch_device(&mut cfg, &mut undo, offer_choice(&offered), None);
        assert_eq!(cfg.device.as_deref(), Some("razer/auto"));
        assert_eq!(undo, None);
        assert_eq!(target_of(cfg.device.as_deref()), Target::Razer);
    }

    #[test]
    fn battery_chart_follows_period() {
        let now = 100 * DAY_SECS;
        let samples = [stats::BatterySample { t: now - 3 * DAY_SECS, pct: 80, charging: false }, stats::BatterySample { t: now, pct: 60, charging: false }];

        // Today: only the last 24 h, hour labels ending with "now".
        let (pts, ticks, since) = chart(&samples, now, Period::Today);
        assert_eq!(pts, [(1.0, 60.0)]);
        assert_eq!(ticks.last().unwrap().1, tr(T::ChartNow));
        assert_eq!(since, None);

        // 7 days, 3 days of history: the axis starts at the first sample,
        // a label per day of it, and "collecting since" that day.
        let (pts, ticks, since) = chart(&samples, now, Period::Week);
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0].0, 0.0);
        assert_eq!(ticks.len(), 4);
        assert_eq!(since, Some(day_label(3)));

        // All: spans back to the oldest sample.
        let (pts, ticks, since) = chart(&samples, now, Period::All);
        assert_eq!(pts[0].0, 0.0);
        assert_eq!(ticks.len(), 4);
        assert_eq!(since, None);
    }

    #[test]
    fn lost_restore_is_sent_again() {
        let r = Restore { dpi: Some(1600), rate: Some(1000), tries: RESTORE_TRIES };
        // The mouse still runs at the test values: both go out again.
        let cmds = restore_cmds(&r, Some(550), Some(125));
        assert!(matches!(cmds[..], [Command::SetBasicDpi(1600), Command::SetBasicRate(1000)]));
        // Back to normal, or not reported: nothing to send.
        assert!(restore_cmds(&r, Some(1600), Some(1000)).is_empty());
        assert!(restore_cmds(&r, None, None).is_empty());
    }

    #[test]
    fn verified_m705_is_not_experimental() {
        let logitech = devices::by_id("logitech/auto").unwrap();
        let ident = |name: &str| Some(Ident { name: name.into(), ..Ident::default() });
        let facts = |id| MouseFacts { model: Some(logitech), info: None, ident: id, status: None, name: String::new(), connected: true };

        let m705 = device_rows(&facts(ident("Logitech Marathon Mouse/Performance Plus M705")));
        assert!(!m705.experimental);
        assert!(m705.rows.contains(&(T::DevSupport, tr(T::BadgeFull).into())));

        assert!(device_rows(&facts(ident("G502 HERO"))).experimental);
        // Not yet identified: still experimental.
        assert!(device_rows(&facts(None)).experimental);
    }

    #[test]
    fn receiver_firmware_is_not_the_mouses() {
        const UNIFYING: u16 = 0xC52B;
        let logitech = devices::by_id("logitech/auto").unwrap();
        let info = DeviceInfo { vid: logitech.vid, pid: UNIFYING, version: 0x1203, ..DeviceInfo::default() };
        let facts = |fw: &str| MouseFacts {
            model: Some(logitech),
            info: Some(info.clone()),
            ident: Some(Ident { firmware: fw.into(), ..Ident::default() }),
            status: None,
            name: String::new(),
            connected: true,
        };

        // Only the receiver's bcdDevice known: labelled, not on Overview.
        let d = device_rows(&facts(""));
        assert_eq!(d.firmware, "");
        assert!(d.rows.contains(&(T::DevFirmware, fill(tr(T::ReceiverFw), &["12.03"]))));

        // The mouse reported its own.
        let d = device_rows(&facts("RQM 17.01 build 0019"));
        assert_eq!(d.firmware, "RQM 17.01 build 0019");
    }

    #[test]
    fn language_switch_relabels_mouse_rows() {
        let lionel = devices::default_model();
        let facts = MouseFacts { model: Some(lionel), info: None, ident: None, status: None, name: lionel.full_name(), connected: false };
        let mut v = initial_view(&AppConfig::default(), false);
        v.device = device_rows(&facts);

        // Settings › Mouse after switching to Slovak at runtime.
        i18n::with_lang(Lang::Sk, || relabel(&mut v, T::LinkConnected, &facts));
        let support = v.device.rows.iter().find(|(k, _)| *k == T::DevSupport).map(|(_, s)| s.clone());
        let slovak = i18n::with_lang(Lang::Sk, || tr(T::BadgeFull).to_string());
        assert_ne!(slovak, tr(T::BadgeFull));
        assert_eq!(support, Some(slovak));
    }

    #[test]
    fn support_text_hides_serial_and_bluetooth_address() {
        let rows = vec![
            (T::DevModel, "Lionel".to_string()),
            (T::DevConnection, "Bluetooth LE · B6:6B:18:D7:1E:00".to_string()),
            (T::DevSerial, "SN12345678".to_string()),
        ];
        let text = support_text(&rows, "01 10", "Windows 11 26300");
        assert!(!text.contains("SN12345678"));
        assert!(text.contains("Bluetooth LE · XX:XX:XX:XX:1E:00"));
        assert!(!text.contains("B6:6B"));
        assert!(text.contains("Lionel") && text.contains("01 10"));
        assert!(text.contains("OS: Windows 11 26300"));
    }

    #[test]
    fn enter_ignores_focus_from_another_page() {
        // Tab to "Reset all buttons", Ctrl+1, Enter: must not reset.
        let ctrl = Mods { ctrl: true, ..Mods::default() };
        let caps = devices::default_model().caps;
        let focus = Some(Focus { index: 1, act: Act::ResetButtons });
        let buttons_acts = [Act::Nav(Page::Overview), Act::ResetButtons];
        assert_eq!(key_action(VK_1, ctrl, focus, &buttons_acts, &caps, Cover::Free), Key::Page(Page::Overview));

        let overview_acts = [Act::Nav(Page::Overview), Act::Activate(0)];
        let enter = key_action(VK_RETURN, Mods::default(), focus, &overview_acts, &caps, Cover::Free);
        assert_ne!(enter, Key::Press(Act::ResetButtons));
    }

    #[test]
    fn later_button_edit_drops_undo() {
        let mut cfg = AppConfig::default();
        cfg.buttons[2] = Action::Disable;
        let mut undo = reset_layout(&mut cfg);

        let mut edited = cfg.buttons;
        edited[3] = Action::Disable;
        assert_eq!(store_layout(&mut cfg, &mut undo, edited), Layout::Stored);
        assert_eq!(ctrl_z(&mut cfg, &mut undo, Cover::Free), None);
        assert_eq!(cfg.buttons, edited);
    }

    #[test]
    fn mouse_switch_drops_undo() {
        let mut cfg = AppConfig::default();
        cfg.buttons[2] = Action::Disable;
        let mut undo = reset_layout(&mut cfg);
        switch_device(&mut cfg, &mut undo, store::GENERIC_DEVICE.into(), None);
        assert_eq!(ctrl_z(&mut cfg, &mut undo, Cover::Free), None);
        assert_eq!(cfg.buttons, DEFAULT_ACTIONS);
    }

    #[test]
    fn ctrl_z_waits_while_a_dialog_is_open() {
        let mut cfg = AppConfig::default();
        cfg.buttons[2] = Action::Disable;
        let mut undo = reset_layout(&mut cfg);
        assert_eq!(ctrl_z(&mut cfg, &mut undo, Cover::Modal), None);
        assert!(undo.is_some(), "kept for after the dialog");
        assert_eq!(ctrl_z(&mut cfg, &mut undo, Cover::Free), Some(Resend::Buttons));
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
