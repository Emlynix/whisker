//! Update service: asks the release channel for a newer version,
//! downloads it, verifies its SHA-256 and swaps it in for the running exe.
//! The app's only network traffic: plain HTTPS GETs with a generic
//! `Whisker/<version>` User-Agent, nothing about the user or the mouse.
//!
//! ```text
//!   check():    latest.json (emlynix.com) ──404/error──> GitHub releases/latest
//!                    └── newer than this build + asset for this platform? -> Newer
//!   download(): asset url (allowlisted host) -> bytes, SHA-256 must match
//!   install():  exe.new (verified again on disk) -> exe -> exe.old,
//!               exe.new -> exe, start it with UPDATED_ARG; cleanup() drops exe.old
//! ```
//!
//! Primary channel, `latest.json`:
//!
//! ```text
//! {
//!   "version": "0.9.1",                    major.minor.patch, optional "v"
//!   "date": "2026-11-20",                  YYYY-MM-DD
//!   "notes": "plain text changelog",       shown as plain text, capped
//!   "assets": {
//!     "windows-x86_64":  { "url": "https://…/whisker.exe", "sha256": "<64 hex>" },
//!     "macos-universal": { "url": "https://…/whisker-macos.zip", "sha256": "<64 hex>" }
//!   }
//! }
//! ```
//!
//! GitHub fallback: `tag_name` ("v0.9.1"), `published_at`, `body` (notes),
//! the asset named `Target::asset` with `browser_download_url` and
//! `digest` ("sha256:<hex>"). A private repository answers 404: no info.
//!
//! Every request is HTTPS to port 443; redirects are followed here (at most
//! `MAX_REDIRECTS`), each hop again HTTPS and on the purpose's host list.

mod json;
mod sha256;

use crate::platform::{self, Fetch, NetError};
use json::Json;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MANIFEST_URL: &str = "https://www.emlynix.com/whisker/latest.json";
const GITHUB_URL: &str = "https://api.github.com/repos/Emlynix/whisker/releases/latest";
const AGENT: &str = concat!("Whisker/", env!("CARGO_PKG_VERSION"));

const JSON_MAX: usize = 64 * 1024;
const BINARY_MAX: usize = 20 * 1024 * 1024;
const JSON_TIMEOUT: Duration = Duration::from_secs(20);
const BINARY_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_REDIRECTS: usize = 5;
/// Changelog characters kept; the dialog shows fewer.
const NOTES_MAX: usize = 4000;
/// `YYYY-MM-DD`.
const DATE_LEN: usize = 10;
const DIGEST_PREFIX: &str = "sha256:";

const HTTP_OK: u16 = 200;
const HTTP_NOT_FOUND: u16 = 404;
const HTTP_REDIRECTS: [u16; 5] = [301, 302, 303, 307, 308];

/// Hosts the manifests may come from (redirects included). The site
/// redirects `www.emlynix.com` to `emlynix.com`.
const MANIFEST_HOSTS: [&str; 3] = ["www.emlynix.com", "emlynix.com", "api.github.com"];
/// Hosts a binary may be downloaded from (redirects included).
const DOWNLOAD_HOSTS: [&str; 5] =
    ["www.emlynix.com", "emlynix.com", "github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];

/// Suffixes of the files next to the exe during a swap.
const NEW_SUFFIX: &str = ".new";
const OLD_SUFFIX: &str = ".old";

/// How this build is named in the channels.
#[derive(Clone, Copy)]
struct Target {
    /// Key under `assets` in latest.json.
    key: &'static str,
    /// Asset name of a GitHub release.
    asset: &'static str,
}

#[cfg(all(windows, target_arch = "x86_64"))]
const TARGET: Option<Target> = Some(Target { key: "windows-x86_64", asset: "whisker-windows-x86_64.exe" });
#[cfg(target_os = "macos")]
const TARGET: Option<Target> = Some(Target { key: "macos-universal", asset: "whisker-macos-universal.zip" });
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const TARGET: Option<Target> = Some(Target { key: "linux-x86_64", asset: "whisker-linux-x86_64" });
#[cfg(not(any(all(windows, target_arch = "x86_64"), target_os = "macos", all(target_os = "linux", target_arch = "x86_64"))))]
const TARGET: Option<Target> = None;

/// `major.minor.patch`; ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version([u32; 3]);

impl Version {
    /// `"0.9.1"` or `"v0.9.1"`; pre-release suffixes and others are `None`.
    fn parse(s: &str) -> Option<Version> {
        const MAX_DIGITS: usize = 9;
        let s = s.trim();
        let s = s.strip_prefix('v').unwrap_or(s);
        let mut out = [0u32; 3];
        let mut parts = s.split('.');
        for o in out.iter_mut() {
            let p = parts.next()?;
            if p.is_empty() || p.len() > MAX_DIGITS || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            *o = p.parse().ok()?;
        }
        parts.next().is_none().then_some(Version(out))
    }

    /// This build; unreadable means "never older", so nothing is offered.
    fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version([u32::MAX; 3]))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let [a, b, c] = self.0;
        write!(f, "{a}.{b}.{c}")
    }
}

/// `https://host/path?query`, port 443, no user info.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Url {
    host: String,
    /// Path and query, starts with `/`.
    path: String,
}

impl Url {
    fn parse(s: &str) -> Option<Url> {
        const SCHEME: &str = "https://";
        let scheme = s.get(..SCHEME.len())?;
        if !scheme.eq_ignore_ascii_case(SCHEME) {
            return None;
        }
        let rest = &s[SCHEME.len()..];
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let host = rest[..end].to_ascii_lowercase();
        let host_ok = |h: &str| h.split('.').all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
        if !host_ok(&host) {
            return None;
        }
        // Fragments never reach the server.
        let tail = rest[end..].split('#').next().unwrap_or_default();
        let path = if tail.starts_with('/') { tail.to_string() } else { format!("/{tail}") };
        // Printable ASCII only: no spaces, controls or backslashes.
        if !path.bytes().all(|b| b.is_ascii_graphic() && b != b'\\') {
            return None;
        }
        Some(Url { host, path })
    }

    /// Target of a `Location` header: absolute, or a path on this host.
    fn join(&self, location: &str) -> Option<Url> {
        if location.starts_with('/') && !location.starts_with("//") {
            return Url::parse(&format!("https://{}{location}", self.host));
        }
        Url::parse(location)
    }
}

/// What a request is for; decides size, time and allowed hosts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Manifest,
    Binary,
}

impl Purpose {
    fn hosts(self) -> &'static [&'static str] {
        match self {
            Purpose::Manifest => &MANIFEST_HOSTS,
            Purpose::Binary => &DOWNLOAD_HOSTS,
        }
    }

    fn allows(self, u: &Url) -> bool {
        self.hosts().contains(&u.host.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Asset {
    url: Url,
    sha256: [u8; sha256::DIGEST_LEN],
}

/// A release as one channel describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Manifest {
    version: Version,
    date: String,
    notes: String,
    /// `None` when this platform's file is missing or malformed.
    asset: Option<Asset>,
}

/// A newer release with a verified-downloadable file for this platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// `YYYY-MM-DD`, may be empty.
    pub date: String,
    /// Plain text, control characters removed.
    pub notes: String,
    asset: Asset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    Newer(Release),
    Current,
    /// No channel answered with a manifest (e.g. both 404).
    NoInfo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fail {
    Network,
    /// Bad manifest, wrong host, too large.
    Refused,
    /// Download differs from the published SHA-256.
    Mismatch,
    /// The exe's folder can't be written, or the swap failed.
    Replace,
}

/// Asks the channels for the newest release (blocking; call off the UI thread).
pub fn check() -> Result<Check, Fail> {
    let Some(target) = TARGET else {
        return Ok(Check::NoInfo);
    };
    let current = Version::current();
    // `Ok(())`: the primary channel has no manifest (404).
    let primary = match read_manifest(MANIFEST_URL, |t| parse_manifest(t, target.key)) {
        Ok(Some(m)) => return Ok(judge(m, current)),
        Ok(None) => Ok(()),
        Err(e) => Err(e),
    };
    match (primary, read_manifest(GITHUB_URL, |t| parse_github(t, target.asset))) {
        (_, Ok(Some(m))) => Ok(judge(m, current)),
        (Err(e), _) | (Ok(()), Err(e)) => Err(e),
        (Ok(()), Ok(None)) => Ok(Check::NoInfo),
    }
}

/// Offered only when strictly newer and downloadable for this platform.
fn judge(m: Manifest, current: Version) -> Check {
    let Some(asset) = m.asset.filter(|_| m.version > current) else {
        return Check::Current;
    };
    Check::Newer(Release { version: m.version, date: m.date, notes: m.notes, asset })
}

fn read_manifest(url: &str, parse: impl Fn(&str) -> Option<Manifest>) -> Result<Option<Manifest>, Fail> {
    let url = Url::parse(url).ok_or(Fail::Refused)?;
    let Some(body) = fetch(&url, Purpose::Manifest)? else {
        return Ok(None);
    };
    let text = std::str::from_utf8(&body).map_err(|_| Fail::Refused)?;
    parse(text).map(Some).ok_or(Fail::Refused)
}

/// GET with checked redirects; `None` on 404.
fn fetch(url: &Url, purpose: Purpose) -> Result<Option<Vec<u8>>, Fail> {
    let (limit, timeout) = match purpose {
        Purpose::Manifest => (JSON_MAX, JSON_TIMEOUT),
        Purpose::Binary => (BINARY_MAX, BINARY_TIMEOUT),
    };
    let mut url = url.clone();
    for _ in 0..=MAX_REDIRECTS {
        if !purpose.allows(&url) {
            return Err(Fail::Refused);
        }
        let req = Fetch { host: &url.host, path: &url.path, agent: AGENT, limit, timeout };
        let reply = platform::net::get(&req).map_err(|e| match e {
            NetError::TooLarge => Fail::Refused,
            NetError::Failed => Fail::Network,
        })?;
        match reply.status {
            HTTP_OK => return Ok(Some(reply.body)),
            HTTP_NOT_FOUND => return Ok(None),
            s if HTTP_REDIRECTS.contains(&s) => {
                url = reply.location.as_deref().and_then(|l| url.join(l)).ok_or(Fail::Refused)?;
            }
            _ => return Err(Fail::Network),
        }
    }
    Err(Fail::Refused)
}

/// Plain text for the dialog: no controls (but newlines), no bidi
/// overrides, at most `NOTES_MAX` characters.
fn clean_notes(s: &str) -> String {
    let bidi = |c: char| matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}');
    s.replace("\r\n", "\n").replace('\t', " ").chars().filter(|&c| c == '\n' || !(c.is_control() || bidi(c))).take(NOTES_MAX).collect()
}

/// `2026-11-20` (also the start of `2026-11-20T10:00:00Z`), else empty.
fn clean_date(s: &str) -> String {
    let d = s.get(..DATE_LEN).unwrap_or_default();
    let digits = d.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() });
    if digits && d.len() == DATE_LEN { d.to_string() } else { String::new() }
}

fn asset_of(url: Option<&str>, hex: Option<&str>) -> Option<Asset> {
    let url = Url::parse(url?).filter(|u| Purpose::Binary.allows(u))?;
    Some(Asset { url, sha256: sha256::parse_hex(hex?)? })
}

/// latest.json; `key` picks this platform's asset.
fn parse_manifest(text: &str, key: &str) -> Option<Manifest> {
    let j = json::parse(text)?;
    let version = Version::parse(j.get("version")?.str()?)?;
    let a = j.get("assets").and_then(|a| a.get(key));
    let asset = a.and_then(|a| asset_of(a.get("url").and_then(Json::str), a.get("sha256").and_then(Json::str)));
    Some(Manifest {
        version,
        date: j.get("date").and_then(Json::str).map(clean_date).unwrap_or_default(),
        notes: j.get("notes").and_then(Json::str).map(clean_notes).unwrap_or_default(),
        asset,
    })
}

/// GitHub `releases/latest`; `name` picks this platform's asset.
fn parse_github(text: &str, name: &str) -> Option<Manifest> {
    let j = json::parse(text)?;
    let version = Version::parse(j.get("tag_name")?.str()?)?;
    let a = j.get("assets")?.items().iter().find(|a| a.get("name").and_then(Json::str) == Some(name));
    let hex = |a: &Json| a.get("digest").and_then(Json::str).and_then(|d| d.strip_prefix(DIGEST_PREFIX)).map(str::to_string);
    let asset = a.and_then(|a| asset_of(a.get("browser_download_url").and_then(Json::str), hex(a).as_deref()));
    Some(Manifest {
        version,
        date: j.get("published_at").and_then(Json::str).map(clean_date).unwrap_or_default(),
        notes: j.get("body").and_then(Json::str).map(clean_notes).unwrap_or_default(),
        asset,
    })
}

/// Downloads the release file; only bytes matching its SHA-256 come back.
pub fn download(r: &Release) -> Result<Vec<u8>, Fail> {
    let body = fetch(&r.asset.url, Purpose::Binary)?.ok_or(Fail::Network)?;
    if sha256::digest(&body) != r.asset.sha256 {
        return Err(Fail::Mismatch);
    }
    Ok(body)
}

/// `C:\…\Whisker.exe` -> `C:\…\Whisker.exe.new`.
fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Swaps verified `bytes` in for the running exe and starts it; the caller
/// quits right after `Ok`. Everything is put back on failure.
pub fn install(r: &Release, bytes: &[u8]) -> Result<(), Fail> {
    let exe = std::env::current_exe().map_err(|_| Fail::Replace)?;
    swap(&exe, bytes, &r.asset.sha256)?;
    let started = std::process::Command::new(&exe).arg(platform::UPDATED_ARG).spawn();
    if started.is_err() {
        // The new file stays as `.new` for nothing: back to the old exe.
        let _ = std::fs::rename(&exe, sibling(&exe, NEW_SUFFIX));
        let _ = std::fs::rename(sibling(&exe, OLD_SUFFIX), &exe);
        return Err(Fail::Replace);
    }
    Ok(())
}

/// `exe.new` written and verified on disk, `exe` -> `exe.old`, `exe.new` -> `exe`.
fn swap(exe: &Path, bytes: &[u8], sha: &[u8; sha256::DIGEST_LEN]) -> Result<(), Fail> {
    use std::io::Write;
    let (new, old) = (sibling(exe, NEW_SUFFIX), sibling(exe, OLD_SUFFIX));
    let _ = std::fs::remove_file(&new);

    // `create_new`: never write through a file or link planted there.
    let written = std::fs::OpenOptions::new().write(true).create_new(true).open(&new).and_then(|mut f| {
        f.write_all(bytes)?;
        f.sync_all()
    });
    if written.is_err() {
        let _ = std::fs::remove_file(&new);
        return Err(Fail::Replace);
    }
    if std::fs::read(&new).map(|b| sha256::digest(&b)).ok().as_ref() != Some(sha) {
        let _ = std::fs::remove_file(&new);
        return Err(Fail::Mismatch);
    }

    let _ = std::fs::remove_file(&old);
    if std::fs::rename(exe, &old).is_err() {
        let _ = std::fs::remove_file(&new);
        return Err(Fail::Replace);
    }
    if std::fs::rename(&new, exe).is_err() {
        let _ = std::fs::rename(&old, exe);
        let _ = std::fs::remove_file(&new);
        return Err(Fail::Replace);
    }
    Ok(())
}

/// Removes what a finished or failed swap left next to the exe; the old
/// exe stays locked until its process ends, so this retries a while.
pub fn cleanup() {
    const TRIES: u32 = 20;
    const GAP: Duration = Duration::from_millis(500);
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let leftovers = [sibling(&exe, OLD_SUFFIX), sibling(&exe, NEW_SUFFIX)];
    if !leftovers.iter().any(|p| p.exists()) {
        return;
    }
    std::thread::spawn(move || {
        for _ in 0..TRIES {
            for p in &leftovers {
                let _ = std::fs::remove_file(p);
            }
            if !leftovers.iter().any(|p| p.exists()) {
                return;
            }
            std::thread::sleep(GAP);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("v1.0.0") > v("0.99.99"));
        assert!(v("0.9.1") > v("0.9.0"));
        assert_eq!(v("v0.9.0"), v("0.9.0"));
        assert_eq!(v("0.9.1").to_string(), "0.9.1");
        for bad in ["", "0.9", "0.9.1.2", "0.9.x", "0.9.1-beta", "0..1", "+1.0.0", "9999999999.0.0", "v", "0.9.1\n2"] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
        assert!(Version::current() < Version([u32::MAX; 3]));
    }

    #[test]
    fn only_newer_versions_are_offered() {
        let asset = Some(Asset { url: Url::parse("https://github.com/a").unwrap(), sha256: [0; 32] });
        let m = |ver: &str, asset: Option<Asset>| Manifest { version: v(ver), date: String::new(), notes: String::new(), asset };
        assert!(matches!(judge(m("0.9.1", asset.clone()), v("0.9.0")), Check::Newer(_)));
        assert_eq!(judge(m("0.9.0", asset.clone()), v("0.9.0")), Check::Current);
        assert_eq!(judge(m("0.8.9", asset), v("0.9.0")), Check::Current);
        // Newer, but nothing to download for this platform.
        assert_eq!(judge(m("1.0.0", None), v("0.9.0")), Check::Current);
    }

    #[test]
    fn urls_are_https_only() {
        let u = Url::parse("HTTPS://WWW.Emlynix.com/whisker/latest.json?x=1#frag").unwrap();
        assert_eq!((u.host.as_str(), u.path.as_str()), ("www.emlynix.com", "/whisker/latest.json?x=1"));
        assert_eq!(Url::parse("https://github.com").unwrap().path, "/");
        for bad in [
            "http://www.emlynix.com/a",
            "ftp://x/y",
            "https://user@github.com/a",
            "https://github.com:8443/a",
            "https://github.com/a b",
            "https://github.com/a\\b",
            "https://git hub.com/",
            "https://github..com/",
            "https:///a",
            "https://github.com/é",
            "javascript:alert(1)",
            "",
        ] {
            assert_eq!(Url::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn redirects_never_downgrade_or_leave_the_allowlist() {
        let base = Url::parse("https://github.com/Emlynix/whisker/releases/download/v1/w.exe").unwrap();
        let hop = base.join("https://release-assets.githubusercontent.com/x?sig=1&a=%2F").unwrap();
        assert!(Purpose::Binary.allows(&hop));
        assert_eq!(base.join("/other").unwrap().host, "github.com");
        assert_eq!(base.join("http://objects.githubusercontent.com/x"), None);
        assert_eq!(base.join("//evil.example/x"), None);
        let evil = base.join("https://evil.example/w.exe").unwrap();
        assert!(!Purpose::Binary.allows(&evil));
        // Look-alike hosts are not on the list.
        for h in ["https://github.com.evil.example/", "https://evilgithub.com/", "https://api.github.com/x"] {
            assert!(!Purpose::Binary.allows(&Url::parse(h).unwrap()), "{h}");
        }
        assert!(!Purpose::Manifest.allows(&Url::parse("https://objects.githubusercontent.com/x").unwrap()));
    }

    #[test]
    fn latest_json_is_read() {
        let text = format!(
            r#"{{"version":"0.9.1","date":"2026-11-20","notes":"Fixes.\r\n- a\u0007b\u202e","assets":{{
                "windows-x86_64":{{"url":"https://www.emlynix.com/whisker/whisker.exe","sha256":"{HEX}"}},
                "macos-universal":{{"url":"https://evil.example/w.zip","sha256":"{HEX}"}}}}}}"#
        );
        let m = parse_manifest(&text, "windows-x86_64").unwrap();
        assert_eq!((m.version, m.date.as_str(), m.notes.as_str()), (v("0.9.1"), "2026-11-20", "Fixes.\n- ab"));
        assert_eq!(m.asset.unwrap().url.host, "www.emlynix.com");
        // Disallowed host or missing platform: release known, nothing to download.
        assert_eq!(parse_manifest(&text, "macos-universal").unwrap().asset, None);
        assert_eq!(parse_manifest(&text, "linux-x86_64").unwrap().asset, None);
        // Bad hash or version.
        assert_eq!(parse_manifest(&text.replace(HEX, "abc"), "windows-x86_64").unwrap().asset, None);
        assert_eq!(parse_manifest(&text.replace("0.9.1", "0.9.1-rc1"), "windows-x86_64"), None);
        assert_eq!(parse_manifest(r#"{"version":1}"#, "windows-x86_64"), None);
        assert_eq!(parse_manifest("<html>", "windows-x86_64"), None);
    }

    #[test]
    fn github_release_is_read() {
        let text = format!(
            r#"{{"tag_name":"v0.9.2","published_at":"2026-11-21T08:00:00Z","body":"Notes","assets":[
                {{"name":"other.zip","browser_download_url":"https://github.com/o","digest":"sha256:{HEX}"}},
                {{"name":"whisker-windows-x86_64.exe","browser_download_url":"https://github.com/Emlynix/whisker/releases/download/v0.9.2/whisker-windows-x86_64.exe","digest":"sha256:{HEX}"}}]}}"#
        );
        let m = parse_github(&text, "whisker-windows-x86_64.exe").unwrap();
        assert_eq!((m.version, m.date.as_str(), m.notes.as_str()), (v("0.9.2"), "2026-11-21", "Notes"));
        assert_eq!(m.asset.unwrap().sha256, sha256::parse_hex(HEX).unwrap());
        // No digest: can't be verified, so nothing to download.
        let undigested = text.replace("\"digest\"", "\"x\"");
        assert_eq!(parse_github(&undigested, "whisker-windows-x86_64.exe").unwrap().asset, None);
        assert_eq!(parse_github(r#"{"message":"Not Found"}"#, "x"), None);
    }

    #[test]
    fn notes_and_dates_are_cleaned() {
        assert_eq!(clean_date("2026-11-20T10:00:00Z"), "2026-11-20");
        assert_eq!(clean_date("20261120"), "");
        assert_eq!(clean_date("2026-1x-20"), "");
        assert_eq!(clean_notes("a\tb\u{1b}[31m\u{2066}c"), "a b[31mc");
        assert_eq!(clean_notes(&"é".repeat(NOTES_MAX + 10)).chars().count(), NOTES_MAX);
    }

    /// Live network: `cargo test live_ -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_check_and_redirected_download() {
        println!("check: {:?}", check());
        const SMALL_ASSET: &str = "https://github.com/BurntSushi/ripgrep/releases/download/14.1.1/ripgrep-14.1.1-x86_64-pc-windows-msvc.zip.sha256";
        let body = fetch(&Url::parse(SMALL_ASSET).unwrap(), Purpose::Binary).unwrap().unwrap();
        println!("asset: {}", String::from_utf8_lossy(&body));
        assert!(!body.is_empty());
    }

    fn temp_exe(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("whisker-update-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("Whisker.exe");
        std::fs::write(&exe, b"old").unwrap();
        exe
    }

    #[test]
    fn swap_replaces_and_keeps_old() {
        let exe = temp_exe("ok");
        swap(&exe, b"abc", &sha256::digest(b"abc")).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"abc");
        assert_eq!(std::fs::read(sibling(&exe, OLD_SUFFIX)).unwrap(), b"old");
        assert!(!sibling(&exe, NEW_SUFFIX).exists());
        let _ = std::fs::remove_dir_all(exe.parent().unwrap());
    }

    #[test]
    fn swap_refuses_unverified_bytes() {
        let exe = temp_exe("bad");
        assert_eq!(swap(&exe, b"evil", &sha256::digest(b"abc")), Err(Fail::Mismatch));
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
        assert!(!sibling(&exe, NEW_SUFFIX).exists());
        assert!(!sibling(&exe, OLD_SUFFIX).exists());
        let _ = std::fs::remove_dir_all(exe.parent().unwrap());
    }
}
