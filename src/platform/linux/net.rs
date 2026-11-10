//! HTTPS GET and POST via `/usr/bin/curl` with fixed arguments, no shell: HTTPS
//! only, no redirects followed, `~/.curlrc` ignored (`-q`), system
//! certificate store. The status and `Location` come after the body.

use crate::platform::{Fetch, NetError, Reply};
use std::io::{Read, Write};
use std::process::{Command, Stdio};

const CURL: &str = "/usr/bin/curl";
/// Appended to stdout after the body: status code, redirect target.
const TRAILER: &str = "\n%{http_code}\n%{redirect_url}";
/// Room for the trailer past the body limit.
const TRAILER_MAX: usize = 16 * 1024;
/// curl exit code: `--max-filesize` exceeded.
const CURL_TOO_LARGE: i32 = 63;

/// POST: the body comes on stdin, as plain text.
const POST_ARGS: [&str; 4] = ["--data-binary", "@-", "--header", "Content-Type: text/plain; charset=utf-8"];

pub fn get(req: &Fetch) -> Result<Reply, NetError> {
    send(req, None)
}

pub fn post(req: &Fetch, body: &[u8]) -> Result<Reply, NetError> {
    send(req, Some(body))
}

/// One request; with a body it is a POST of plain text.
fn send(req: &Fetch, body: Option<&[u8]>) -> Result<Reply, NetError> {
    let url = format!("https://{}{}", req.host, req.path);
    let secs = req.timeout.as_secs().max(1).to_string();
    let limit = req.limit.to_string();
    // `-q` must come first to skip ~/.curlrc.
    let args = [
        "-q", "--silent", "--globoff", "--proto", "=https", "--max-redirs", "0", "--max-time", &secs, "--max-filesize", &limit,
        "--user-agent", req.agent, "--write-out", TRAILER, &url,
    ];
    let mut cmd = Command::new(CURL);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    if body.is_some() {
        cmd.args(POST_ARGS).stdin(Stdio::piped());
    }
    let mut child = cmd.spawn().map_err(|_| NetError::Failed)?;
    // curl reads the whole body before it connects; closing stdin ends it.
    if let (Some(b), Some(mut stdin)) = (body, child.stdin.take()) {
        let _ = stdin.write_all(b);
    }

    // Bounded read: a body without a length could otherwise grow forever.
    let mut out = Vec::new();
    let cap = (req.limit + TRAILER_MAX) as u64;
    let read = child.stdout.take().map(|s| s.take(cap + 1).read_to_end(&mut out));
    if out.len() as u64 > cap {
        let _ = child.kill();
        let _ = child.wait();
        return Err(NetError::TooLarge);
    }
    let status = child.wait().map_err(|_| NetError::Failed)?;
    if status.code() == Some(CURL_TOO_LARGE) {
        return Err(NetError::TooLarge);
    }
    if !matches!(read, Some(Ok(_))) || !status.success() {
        return Err(NetError::Failed);
    }

    let reply = split_trailer(out).ok_or(NetError::Failed)?;
    if reply.body.len() > req.limit {
        return Err(NetError::TooLarge);
    }
    Ok(reply)
}

/// `body \n 302 \n https://…` -> reply; status 000 (no response) is `None`.
fn split_trailer(mut out: Vec<u8>) -> Option<Reply> {
    let nl = out.iter().rposition(|&b| b == b'\n')?;
    let location = String::from_utf8(out[nl + 1..].to_vec()).ok().filter(|l| !l.is_empty());
    out.truncate(nl);
    let nl = out.iter().rposition(|&b| b == b'\n')?;
    let status = std::str::from_utf8(&out[nl + 1..]).ok()?.parse::<u16>().ok().filter(|&s| s > 0)?;
    out.truncate(nl);
    Some(Reply { status, location, body: out })
}

#[cfg(test)]
mod tests {
    use super::split_trailer;

    #[test]
    fn trailer_splits_off_the_body() {
        let r = split_trailer(b"{\"a\":\n1}\n200\n".to_vec()).unwrap();
        assert_eq!((r.status, r.location, r.body), (200, None, b"{\"a\":\n1}".to_vec()));
        let r = split_trailer(b"\n302\nhttps://x/y".to_vec()).unwrap();
        assert_eq!((r.status, r.location.as_deref()), (302, Some("https://x/y")));
        assert!(split_trailer(b"\n000\n".to_vec()).is_none());
        assert!(split_trailer(b"x".to_vec()).is_none());
    }
}
