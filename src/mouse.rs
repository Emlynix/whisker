//! Mouse service: owns the HID link on its own thread. Callers send
//! commands and receive status events; they never touch HID directly.
//!
//! ```text
//!  UI/controller --Command--> [mouse thread] --frames--> HidLink --> mouse
//!        ^                          |
//!        +-------Event (status)-----+
//! ```

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
    /// Window visibility drives how often status is polled.
    Visible(bool),
    RefreshNow,
}

pub enum Event {
    /// The configuration interface was (re)opened: identity of the device.
    Device(DeviceInfo),
    Status(Status),
    /// Dongle present but mouse asleep or off.
    Offline,
    /// No receiver or cable found.
    Absent,
}

pub struct MouseService {
    tx: Sender<Command>,
}

impl MouseService {
    pub fn start(on_event: impl Fn(Event) + Send + 'static) -> Self {
        let (tx, rx) = channel();
        thread::Builder::new()
            .name("mouse".into())
            .spawn(move || run(rx, on_event))
            .expect("spawn mouse thread");
        Self { tx }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
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

fn run(rx: Receiver<Command>, on_event: impl Fn(Event)) {
    let mut w = Worker { link: Some(HidLink::new()), seq: 0, verified: false, reported: None };
    let mut period = POLL_VISIBLE;

    loop {
        let status = w.poll();
        let info = w.link.as_ref().and_then(|l| l.info().cloned());
        if info.is_some() && info != w.reported {
            w.reported = info.clone();
            on_event(Event::Device(info.unwrap_or_default()));
        }
        on_event(status);

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
            Command::Visible(v) => period = if v { POLL_VISIBLE } else { POLL_HIDDEN },
            Command::RefreshNow => {}
        }
    }
}
