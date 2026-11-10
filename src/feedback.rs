//! Sends a report to the developers, only when the user clicks "Send"
//! on a text they see in full (a mouse report or the diagnostic log).
//!
//! ```text
//! POST https://whisker.emlynix.com/api/report?kind=mouse|log
//! Content-Type: text/plain; charset=utf-8
//! body: the text as shown, at most 128 KB
//!
//! 200 or 201, body: the report id as plain text, e.g. WR-7K3F9Q
//! (4 to 32 characters: A-Z, a-z, 0-9, '-')
//! ```
//! The id is what the user quotes in a GitHub issue. Any other status,
//! a redirect, or a reply that is no id counts as not sent.

use crate::platform::{Fetch, NetError, net};
use std::time::Duration;

const HOST: &str = "whisker.emlynix.com";
const PATH: &str = "/api/report";
const AGENT: &str = concat!("Whisker/", env!("CARGO_PKG_VERSION"));
/// The service refuses more; the texts sent are far smaller.
const BODY_MAX: usize = 128 * 1024;
/// Room for an error page, so its status is still read.
const REPLY_MAX: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);
const ID_LEN: std::ops::RangeInclusive<usize> = 4..=32;
const HTTP_OK: u16 = 200;
const HTTP_CREATED: u16 = 201;

/// What is being sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// "Help add your mouse" report.
    Mouse,
    /// Diagnostic log after a crash or hang.
    Log,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fail {
    TooLarge,
    /// No connection, TLS error, timeout.
    Network,
    /// The service answered with this status.
    Refused(u16),
    /// Accepted, but the reply is no report id.
    BadReply,
}

/// Sends `text`; the report id on success. Blocking.
pub fn send(kind: Kind, text: &str) -> Result<String, Fail> {
    if text.len() > BODY_MAX {
        return Err(Fail::TooLarge);
    }
    let path = path_of(kind);
    let req = Fetch { host: HOST, path: &path, agent: AGENT, limit: REPLY_MAX, timeout: TIMEOUT };
    let reply = net::post(&req, text.as_bytes()).map_err(|e| match e {
        NetError::Failed => Fail::Network,
        NetError::TooLarge => Fail::BadReply,
    })?;

    if !matches!(reply.status, HTTP_OK | HTTP_CREATED) {
        return Err(Fail::Refused(reply.status));
    }
    report_id(&reply.body).ok_or(Fail::BadReply)
}

fn path_of(kind: Kind) -> String {
    let kind = match kind {
        Kind::Mouse => "mouse",
        Kind::Log => "log",
    };
    format!("{PATH}?kind={kind}")
}

/// The id in a reply: shown to the user, so nothing but plain letters,
/// digits and dashes is taken from the network.
fn report_id(body: &[u8]) -> Option<String> {
    let id = std::str::from_utf8(body).ok()?.trim();
    let plain = id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    (ID_LEN.contains(&id.len()) && plain).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_id_is_taken_from_the_reply() {
        assert_eq!(report_id(b"WR-7K3F9Q\n").as_deref(), Some("WR-7K3F9Q"));
        assert_eq!(report_id(b"abc"), None);
        assert_eq!(report_id(b"<html>Not found</html>"), None);
        assert_eq!(report_id(b"WR 7K3F9Q"), None);
        assert_eq!(report_id(&[b'A'; 33]), None);
        assert_eq!(report_id(&[0xFF, 0xFE, 0x41, 0x42]), None);
    }

    /// Live request to the service: `cargo test -- --ignored --nocapture live_send`.
    #[test]
    #[ignore]
    fn live_send() {
        println!("{:?}", send(Kind::Log, "test from the Whisker test suite"));
    }

    #[test]
    fn kind_goes_into_the_query() {
        assert_eq!(path_of(Kind::Mouse), "/api/report?kind=mouse");
        assert_eq!(path_of(Kind::Log), "/api/report?kind=log");
    }

    #[test]
    fn oversized_text_is_never_sent() {
        let text = "x".repeat(BODY_MAX + 1);
        assert_eq!(send(Kind::Log, &text), Err(Fail::TooLarge));
    }
}
