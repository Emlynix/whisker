//! Mouse service: owns the HID link on its own thread. Callers send
//! commands and receive status events; they never touch HID directly.
//!
//! ```text
//!  UI/controller --Command--> [mouse thread] --> Lionel HidLink
//!        ^                          |        \-> drivers::{Hidpp, Razer}
//!        +-------Event (status)-----+
//! ```

use crate::drivers::eternico::Eternico;
use crate::drivers::hidpp::Hidpp;
use crate::drivers::oemhid::OemHid;
use crate::drivers::roccat::Roccat;
use crate::drivers::corsair::Corsair;
use crate::drivers::asus::Asus;
use crate::drivers::razer::Razer;
use crate::drivers::sinowealth::Sinowealth;
use crate::drivers::steelseries::SteelSeries;
use crate::drivers::{Ability, Ident, Pick, Reading};
use crate::hid_link::{DeviceInfo, HidLink};
use crate::protocol::{self, Action, BUTTON_COUNT, DPI_COLOR_SLOTS, DpiConfig, Lighting, Lod, MacroEvent, PollingRate, Power, Status};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::Duration;

/// Gap between consecutive writes; vendor app waits 60 ms.
const WRITE_GAP: Duration = Duration::from_millis(60);
/// Firmware may drop a rate change sent too soon after another; resend once.
const POLLING_RESEND: Duration = Duration::from_millis(500);
const POLL_VISIBLE: Duration = Duration::from_secs(2);
const POLL_HIDDEN: Duration = Duration::from_secs(60);
/// Unanswered polls before a driver's channel is reopened.
const MAX_MISSES: u8 = 3;

/// Which driver talks to the mouse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Lionel,
    Hidpp,
    Razer,
    /// Eternico receiver, listen-only.
    Eternico,
    Sinowealth,
    SteelSeries,
    OemHid,
    Asus,
    Corsair,
    Roccat,
    /// Bluetooth LE mouse without a driver: battery from Windows only.
    Bluetooth(String),
    /// Mouse without a driver: statistics only.
    Nothing,
}

pub enum Command {
    SetDpi(DpiConfig),
    SetPolling(PollingRate),
    SetLighting(Lighting),
    SetDpiColors([u32; DPI_COLOR_SLOTS]),
    SetDebounce(u8),
    SetLod(Lod),
    SetPower(Power),
    /// All button actions plus the macro list they may reference.
    SetButtons([Action; BUTTON_COUNT], Vec<Vec<MacroEvent>>),
    /// Switch driver (another mouse was chosen) and which devices it may open.
    Target(Target, Pick),
    /// DPI / report rate for driver-based mice.
    SetBasicDpi(u32),
    SetBasicRate(u32),
    /// Window visibility drives how often status is polled.
    Visible(bool),
    RefreshNow,
    /// Read-only probe of one device by each of these drivers (report wizard).
    Probe(Vec<Target>, Pick),
    /// Answers once every earlier command is done (before quitting).
    Flush(Sender<()>),
}

pub enum Event {
    /// The configuration interface was (re)opened: identity of the device.
    Device(DeviceInfo),
    Status(Status),
    /// Driver-based mouse opened: identity and abilities.
    Ident(Ident, Ability),
    /// Status of a driver-based mouse.
    Basic(Reading),
    /// Dongle present but mouse asleep or off.
    Offline,
    /// No receiver or cable found.
    Absent,
    /// Results of `Command::Probe` for that pick, one per driver.
    Probe(Pick, Vec<Probe>),
}

/// What one driver learned in a read-only probe.
#[derive(Clone, Debug)]
pub struct Probe {
    pub target: Target,
    pub got: Probed,
}

#[derive(Clone, Debug)]
pub enum Probed {
    /// Opened: identity, abilities and one status reading (if answered).
    Driver(DeviceInfo, Ident, Ability, Option<Reading>),
    /// Lionel family: the status reply, if one came.
    Status(DeviceInfo, Option<Status>),
    /// The driver did not accept the device.
    Refused,
}

pub struct MouseService {
    tx: Sender<Command>,
}

impl MouseService {
    pub fn start(target: Target, pick: Pick, on_event: impl Fn(Event) + Send + 'static) -> Self {
        let (tx, rx) = channel();
        thread::Builder::new()
            .name("mouse".into())
            .spawn(move || run(target, pick, rx, on_event))
            .expect("spawn mouse thread");
        Self { tx }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    /// Waits up to `limit` until the commands sent so far are carried out,
    /// e.g. settings put back just before the app quits.
    pub fn flush(&self, limit: Duration) {
        let (tx, rx) = channel();
        self.send(Command::Flush(tx));
        let _ = rx.recv_timeout(limit);
    }
}

/// Opened driver for a non-Lionel mouse.
enum Driver {
    Hidpp(Hidpp),
    Razer(Razer),
    Eternico(Eternico),
    Sinowealth(Sinowealth),
    SteelSeries(SteelSeries),
    OemHid(OemHid),
    Asus(Asus),
    Corsair(Corsair),
    Roccat(Roccat),
}

impl Driver {
    /// Opens the driver of `t` on a device `pick` allows.
    fn open(t: &Target, pick: &Pick) -> Option<(Driver, DeviceInfo)> {
        match t {
            Target::Hidpp => Hidpp::open(pick).map(|(d, i)| (Driver::Hidpp(d), i)),
            Target::Razer => Razer::open(pick).map(|(d, i)| (Driver::Razer(d), i)),
            Target::Eternico => Eternico::open(pick).map(|(d, i)| (Driver::Eternico(d), i)),
            Target::Sinowealth => Sinowealth::open(pick).map(|(d, i)| (Driver::Sinowealth(d), i)),
            Target::SteelSeries => SteelSeries::open(pick).map(|(d, i)| (Driver::SteelSeries(d), i)),
            Target::OemHid => OemHid::open(pick).map(|(d, i)| (Driver::OemHid(d), i)),
            Target::Roccat => Roccat::open(pick).map(|(d, i)| (Driver::Roccat(d), i)),
            Target::Corsair => Corsair::open(pick).map(|(d, i)| (Driver::Corsair(d), i)),
            Target::Asus => Asus::open(pick).map(|(d, i)| (Driver::Asus(d), i)),
            _ => None,
        }
    }

    fn describe(&mut self) -> (Ident, Ability) {
        match self {
            Driver::Hidpp(d) => (d.ident(), d.ability()),
            Driver::Razer(d) => (d.ident(), d.ability()),
            Driver::Eternico(d) => (d.ident(), d.ability()),
            Driver::Sinowealth(d) => (d.ident(), d.ability()),
            Driver::SteelSeries(d) => (d.ident(), d.ability()),
            Driver::OemHid(d) => (d.ident(), d.ability()),
            Driver::Roccat(d) => (d.ident(), d.ability()),
            Driver::Corsair(d) => (d.ident(), d.ability()),
            Driver::Asus(d) => (d.ident(), d.ability()),
        }
    }

    fn read(&mut self) -> Option<Reading> {
        match self {
            Driver::Hidpp(d) => d.read(),
            Driver::Razer(d) => d.read(),
            Driver::Eternico(d) => d.read(),
            Driver::Sinowealth(d) => d.read(),
            Driver::SteelSeries(d) => d.read(),
            Driver::OemHid(d) => d.read(),
            Driver::Roccat(d) => d.read(),
            Driver::Corsair(d) => d.read(),
            Driver::Asus(d) => d.read(),
        }
    }

    fn set_dpi(&mut self, dpi: u32) {
        match self {
            Driver::Hidpp(d) => d.set_dpi(dpi),
            Driver::Razer(d) => d.set_dpi(dpi),
            // Listen-only.
            Driver::Eternico(_) => false,
            Driver::Sinowealth(d) => d.set_dpi(dpi),
            Driver::SteelSeries(d) => d.set_dpi(dpi),
            Driver::OemHid(d) => d.set_dpi(dpi),
            Driver::Roccat(d) => d.set_dpi(dpi),
            Driver::Corsair(d) => d.set_dpi(dpi),
            Driver::Asus(d) => d.set_dpi(dpi),
        };
    }

    fn set_rate(&mut self, hz: u32) {
        match self {
            Driver::Hidpp(d) => d.set_rate(hz),
            Driver::Razer(d) => d.set_rate(hz),
            Driver::Eternico(_) => false,
            Driver::Sinowealth(d) => d.set_rate(hz),
            Driver::SteelSeries(d) => d.set_rate(hz),
            Driver::OemHid(d) => d.set_rate(hz),
            Driver::Roccat(d) => d.set_rate(hz),
            Driver::Corsair(d) => d.set_rate(hz),
            Driver::Asus(d) => d.set_rate(hz),
        };
    }
}

struct Worker {
    link: Option<HidLink>,
    seq: u8,
    /// A status reply with the Lionel signature was seen on this link.
    /// Writes are refused until then, so look-alike devices are never configured.
    verified: bool,
    /// Last identity reported, so `Device` is sent only on change.
    reported: Option<DeviceInfo>,
    target: Target,
    pick: Pick,
    driver: Option<Driver>,
    misses: u8,
}

impl Worker {
    fn next_seq(&mut self) -> u8 {
        let s = self.seq;
        self.seq = self.seq.wrapping_add(1);
        s
    }

    fn write(&mut self, frame: [u8; protocol::REPORT_LEN]) {
        if !self.verified {
            return;
        }
        if let Some(link) = self.link.as_mut() {
            link.send(&frame);
        }
        thread::sleep(WRITE_GAP);
    }

    fn poll(&mut self) -> Event {
        let seq = self.next_seq();
        let Some(link) = self.link.as_mut() else {
            return Event::Absent;
        };

        if !link.is_present() {
            self.verified = false;
            return Event::Absent;
        }

        match link.query(&protocol::status_request(seq)).and_then(|r| protocol::parse_status(&r)) {
            Some(s) => {
                self.verified = true;
                Event::Status(s)
            }
            None => Event::Offline,
        }
    }
}

impl Worker {
    /// Poll of a driver-based mouse; opens the driver first if needed.
    fn poll_driver(&mut self, on_event: &impl Fn(Event)) {
        if self.driver.is_none() {
            let Some((mut d, info)) = Driver::open(&self.target, &self.pick) else {
                on_event(Event::Absent);
                return;
            };
            on_event(Event::Device(info));
            let (ident, ability) = d.describe();
            on_event(Event::Ident(ident, ability));
            self.driver = Some(d);
            self.misses = 0;
        }

        let Some(d) = self.driver.as_mut() else {
            return;
        };
        match d.read() {
            Some(r) => {
                self.misses = 0;
                on_event(Event::Basic(r));
            }
            None => {
                // Asleep, or unplugged: reopen after a few misses.
                self.misses += 1;
                if self.misses >= MAX_MISSES {
                    self.driver = None;
                }
                on_event(Event::Offline);
            }
        }
    }

    fn poll_lionel(&mut self, on_event: &impl Fn(Event)) {
        let status = self.poll();
        let info = self.link.as_ref().and_then(|l| l.info().cloned());
        if info.is_some() && info != self.reported {
            self.reported = info.clone();
            on_event(Event::Device(info.unwrap_or_default()));
        }
        on_event(status);
    }
}

/// Opens the device with each driver on its own channel, reads identity
/// and one status, closes it again. Only the drivers' identify and read
/// requests are sent, never a setting.
fn probe(targets: &[Target], pick: &Pick) -> Vec<Probe> {
    let mut out = Vec::new();
    for t in targets {
        let got = match t {
            Target::Lionel => probe_lionel(pick),
            _ => match Driver::open(t, pick) {
                Some((mut d, info)) => {
                    let (ident, ability) = d.describe();
                    Probed::Driver(info, ident, ability, d.read())
                }
                None => Probed::Refused,
            },
        };
        out.push(Probe { target: t.clone(), got });
    }
    out
}

/// Lionel family: one status request on the vendor's config collection.
fn probe_lionel(pick: &Pick) -> Probed {
    let mut link = HidLink::new();
    let Some(info) = link.is_present().then(|| link.info().cloned()).flatten() else {
        return Probed::Refused;
    };
    if !pick.allows(info.vid, info.pid) {
        return Probed::Refused;
    }
    let status = link.query(&protocol::status_request(0)).and_then(|r| protocol::parse_status(&r));
    Probed::Status(info, status)
}

fn run(target: Target, pick: Pick, rx: Receiver<Command>, on_event: impl Fn(Event)) {
    let mut w = Worker { link: Some(HidLink::new()), seq: 0, verified: false, reported: None, target, pick, driver: None, misses: 0 };
    let mut period = POLL_VISIBLE;

    loop {
        match &w.target {
            Target::Lionel => w.poll_lionel(&on_event),
            Target::Hidpp | Target::Razer | Target::Eternico | Target::Sinowealth | Target::SteelSeries | Target::OemHid | Target::Asus | Target::Corsair | Target::Roccat => {
                w.poll_driver(&on_event)
            }
            Target::Bluetooth(addr) => match crate::platform::bluetooth::battery(addr) {
                Some(pct) => on_event(Event::Basic(Reading { battery: Some(pct), ..Reading::default() })),
                None => on_event(Event::Absent),
            },
            Target::Nothing => {}
        }

        // Wait for a command or the next poll tick.
        let cmd = match rx.recv_timeout(period) {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };

        match cmd {
            Command::SetDpi(cfg) => {
                let seq = w.next_seq();
                w.write(protocol::dpi_frame(&cfg, seq));
            }
            Command::SetPolling(rate) => {
                let seq = w.next_seq();
                w.write(protocol::polling_frame(rate, seq));
                thread::sleep(POLLING_RESEND);
                let seq = w.next_seq();
                w.write(protocol::polling_frame(rate, seq));
            }
            Command::SetLighting(l) => {
                let seq = w.next_seq();
                w.write(protocol::lighting_frame(&l, seq));
            }
            Command::SetDpiColors(c) => {
                let seq = w.next_seq();
                w.write(protocol::dpi_colors_frame(&c, seq));
            }
            Command::SetDebounce(ms) => {
                let seq = w.next_seq();
                w.write(protocol::debounce_frame(ms, seq));
            }
            Command::SetLod(lod) => {
                let seq = w.next_seq();
                w.write(protocol::lod_frame(lod, seq));
            }
            Command::SetPower(p) => {
                let seq = w.next_seq();
                w.write(protocol::power_frame(&p, seq));
            }
            Command::SetButtons(actions, macros) => {
                let frames = protocol::button_frames(&actions, &macros, || w.next_seq());
                for f in frames {
                    w.write(f);
                }
            }
            Command::Target(t, pick) => {
                (w.target, w.pick) = (t, pick);
                w.driver = None;
                w.verified = false;
                w.reported = None;
            }
            Command::SetBasicDpi(dpi) => {
                if let Some(d) = w.driver.as_mut() {
                    d.set_dpi(dpi);
                }
            }
            Command::SetBasicRate(hz) => {
                if let Some(d) = w.driver.as_mut() {
                    d.set_rate(hz);
                }
            }
            Command::Visible(v) => period = if v { POLL_VISIBLE } else { POLL_HIDDEN },
            Command::RefreshNow => {}
            Command::Probe(targets, pick) => {
                let found = probe(&targets, &pick);
                on_event(Event::Probe(pick, found));
            }
            Command::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}
