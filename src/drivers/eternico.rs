//! Eternico AET-MS480B 2.4 GHz receiver (1D57:FA60). Listen-only: the
//! receiver reports on its own, nothing is ever sent to it.
//!
//! ```text
//! collection usage page 0x0A, input report 03 (5 bytes):
//!   03 10 40 01 BB   every ~2 s while the mouse is on; BB = battery / 10
//!   03 00 10 0N 00   DPI button pressed, N = stage 1..4
//! ```
//! Battery meaning is inferred: 09 matched 90 % read over Bluetooth.

use super::{Ability, Ident, Reading};
use crate::hid_link::raw::{Channel, interfaces};
use std::time::Duration;

pub const VID: u16 = 0x1D57;
pub const PID: u16 = 0xFA60;

const REPORT_ID: u8 = 0x03;
const REPORT_LEN: usize = 5;
const STATUS_PAGE: u16 = 0x000A;
const KIND_HEARTBEAT: u8 = 0x10;
const KIND_STAGE: u8 = 0x00;
const HEARTBEAT_MARK: u8 = 0x40;
pub const STAGES: u8 = 4;
const TENTHS: u8 = 10;
/// Heartbeats come every 2 s; a little more covers jitter.
const LISTEN: Duration = Duration::from_millis(2600);
/// Time to pick up reports already queued after the heartbeat.
const DRAIN: Duration = Duration::from_millis(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Report {
    /// Mouse alive, battery in %.
    Heartbeat(u8),
    /// DPI stage 1..=STAGES.
    Stage(u8),
}

pub fn parse(r: &[u8]) -> Option<Report> {
    if r.len() < REPORT_LEN || r[0] != REPORT_ID {
        return None;
    }
    match (r[1], r[2]) {
        (KIND_HEARTBEAT, HEARTBEAT_MARK) => Some(Report::Heartbeat((r[4].min(TENTHS)) * TENTHS)),
        (KIND_STAGE, _) if (1..=STAGES).contains(&r[3]) => Some(Report::Stage(r[3])),
        _ => None,
    }
}

pub struct Eternico {
    ch: Channel,
    stage: Option<u8>,
}

impl Eternico {
    pub fn open() -> Option<(Eternico, crate::hid_link::DeviceInfo)> {
        let iface = interfaces(VID).into_iter().find(|i| i.info.pid == PID && i.usage_page == STATUS_PAGE)?;
        let ch = Channel::open(&iface.path)?;
        Some((Eternico { ch, stage: None }, iface.info))
    }

    pub fn ident(&mut self) -> Ident {
        Ident { protocol: "Eternico 2.4 GHz (listen-only)".into(), ..Ident::default() }
    }

    pub fn ability(&mut self) -> Ability {
        Ability { battery: true, stages: Some(STAGES), ..Ability::default() }
    }

    /// Drains queued reports and waits for one heartbeat; `None` when the
    /// mouse is off (no heartbeat).
    pub fn read(&mut self) -> Option<Reading> {
        let mut battery = None;
        let deadline = std::time::Instant::now() + LISTEN;
        loop {
            // Until a heartbeat: wait up to the deadline; after it: only
            // drain what is already queued.
            let wait = match battery {
                Some(_) => DRAIN,
                None => match deadline.checked_duration_since(std::time::Instant::now()) {
                    Some(left) => left,
                    None => break,
                },
            };
            let Some(r) = self.ch.read(REPORT_LEN, wait) else {
                break;
            };
            match parse(&r) {
                Some(Report::Heartbeat(b)) => battery = Some(b),
                Some(Report::Stage(s)) => self.stage = Some(s),
                None => {}
            }
        }
        battery.map(|b| Reading { battery: Some(b), stage: self.stage, ..Reading::default() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_reports() {
        assert_eq!(parse(&[0x03, 0x10, 0x40, 0x01, 0x09]), Some(Report::Heartbeat(90)));
        assert_eq!(parse(&[0x03, 0x10, 0x40, 0x01, 0x0A]), Some(Report::Heartbeat(100)));
        assert_eq!(parse(&[0x03, 0x00, 0x10, 0x03, 0x00]), Some(Report::Stage(3)));
        assert_eq!(parse(&[0x03, 0x10, 0x36, 0xFF, 0x48]), None);
        assert_eq!(parse(&[0x02, 0xE9, 0x00]), None);
    }
}
