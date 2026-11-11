//! App settings on disk (`%APPDATA%\Whisker\config.txt`) and import of the
//! current profile from the vendor app.
//!
//! The mouse cannot report its settings back, so the app keeps the last
//! applied values here. Values from the vendor app form the base; our file
//! overrides them key by key. Format is plain `key=value`:
//!
//! ```text
//! stages=6,12,22,30,51,58
//! enabled=63
//! polling=1000
//! lighting=1,2,2,1          effect id, brightness, speed, direction
//! breathing=FF0000,0000FF
//! debounce=12
//! ```

use crate::i18n::Lang;
use crate::protocol::{
    Action, BREATHING_MAX, BUTTON_COUNT, COLORFUL_SLOTS, DEFAULT_ACTIONS, DPI_COLOR_SLOTS, Direction, DpiConfig, Effect,
    Lighting, Lod, MACRO_COUNT, MACRO_MAX_EVENTS, MacroEvent, PollingRate, Power, STAGE_COUNT,
};
use crate::theme;
use std::path::PathBuf;

const APP_DIR: &str = crate::APP_NAME;
/// Folder of the app's former name, moved once on start.
const OLD_APP_DIR: &str = "Lionel";
const CONFIG_FILE: &str = "config.txt";
pub const STATS_FILE: &str = "stats.txt";
const VENDOR_DIR: &str = "GamingMouseData20230920C\\config";

pub const DEBOUNCE_DEFAULT: u8 = 12;
/// `device=` value for a mouse without a driver (statistics only).
pub const GENERIC_DEVICE: &str = "generic";
pub const SLEEP_DEFAULT: u8 = 6;

#[derive(Clone)]
pub struct AppConfig {
    pub dpi: DpiConfig,
    pub polling: PollingRate,
    pub lighting: Lighting,
    pub dpi_colors: [u32; DPI_COLOR_SLOTS],
    pub debounce: u8,
    pub lod: Lod,
    pub power: Power,
    /// Button actions in device order: L, R, M, Back, Forward, DPI.
    pub buttons: [Action; BUTTON_COUNT],
    pub macros: Vec<Vec<MacroEvent>>,
    /// Chosen mouse model id, `GENERIC_DEVICE` for statistics only, `None`
    /// until the user confirmed one.
    pub device: Option<String>,
    /// VID:PID of the detected mouse the user picked, so a vendor-wide
    /// driver opens that one; `None` = any of the model's products.
    pub device_usb: Option<(u16, u16)>,
    /// Last DPI stage reported by a stage-only mouse (it reports changes only).
    pub last_stage: Option<u8>,
    /// First-run install question was answered.
    pub setup_asked: bool,
    pub theme: theme::Choice,
    /// `None` follows the Windows display language.
    pub lang: Option<Lang>,
    /// Mice answered "Not now" in the new-mouse banner, as `mouse_key`.
    pub ignored_mice: Vec<String>,
    /// Effects: background grid and texture; subtle animations.
    pub fx_texture: bool,
    pub fx_motion: bool,
    /// The window shows the text mode.
    pub tui: bool,
    /// Look for a new version once a day.
    pub update_check: bool,
    /// Unix time of the last completed update check.
    pub update_checked: u64,
    /// Version the user chose to skip, e.g. `"0.9.1"`.
    pub update_skip: Option<String>,
    /// Version already announced by a notification.
    pub update_notified: Option<String>,
}

impl Default for AppConfig {
    /// Vendor factory defaults: 1000/1600/2400/3200/5000/6000 DPI, 500 Hz.
    fn default() -> Self {
        Self {
            dpi: DpiConfig { stages: [8, 14, 22, 30, 48, 50], enabled: 0x3F, active: 2 },
            polling: PollingRate::Hz500,
            lighting: Lighting::default(),
            dpi_colors: [0xFF0000, 0x00FF00, 0x0000FF, 0x00FFFF, 0xFFFF00, 0xFF00FF, 0xFFFFFF, 0xFFFFFF],
            debounce: DEBOUNCE_DEFAULT,
            lod: Lod::Mm2,
            power: Power { sleep_min: SLEEP_DEFAULT, dark_while_moving: true },
            buttons: DEFAULT_ACTIONS,
            macros: vec![Vec::new(); MACRO_COUNT],
            device: None,
            device_usb: None,
            last_stage: None,
            setup_asked: false,
            theme: theme::Choice::System,
            lang: None,
            ignored_mice: Vec::new(),
            fx_texture: true,
            fx_motion: true,
            tui: false,
            update_check: true,
            update_checked: 0,
            update_skip: None,
            update_notified: None,
        }
    }
}

/// `%APPDATA%\Whisker`, resolved by the shell; `None` disables saving.
pub fn data_dir() -> Option<PathBuf> {
    // Debug builds: `WHISKER_DATA=<dir>` keeps demo runs off the real data.
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("WHISKER_DATA") {
        return Some(PathBuf::from(dir));
    }
    Some(crate::platform::config_dir()?.join(APP_DIR))
}

/// Moves settings and statistics from the app's former folder name once.
pub fn migrate_old_dir() {
    let Some(base) = crate::platform::config_dir() else {
        return;
    };
    let (old, new) = (base.join(OLD_APP_DIR), base.join(APP_DIR));
    if old.is_dir() && !new.exists() {
        let _ = std::fs::rename(old, new);
    }
}

/// Writes via temp file + rename so a crash never leaves half a file.
pub fn save_text(name: &str, text: &str) {
    let Some(dir) = data_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let tmp = dir.join(format!("{name}.tmp"));
    if write_private(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join(name));
    }
}

/// Config and statistics are the user's alone: 0600 on Unix.
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        const OWNER_RW: u32 = 0o600;
        // `mode` applies only to a new file: drop a leftover one first.
        let _ = std::fs::remove_file(path);
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(OWNER_RW).open(path)?;
        f.write_all(text.as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, text)
    }
}

pub fn load_text(name: &str) -> Option<String> {
    std::fs::read_to_string(data_dir()?.join(name)).ok()
}

/// Deletes only the files this app writes, then the folder if it is empty.
pub fn remove_data() {
    let Some(dir) = data_dir() else {
        return;
    };
    for name in [CONFIG_FILE, STATS_FILE] {
        let _ = std::fs::remove_file(dir.join(name));
        let _ = std::fs::remove_file(dir.join(format!("{name}.tmp")));
    }
    let _ = std::fs::remove_dir(&dir);
}

/// `key=value` lines as pairs; other lines are ignored.
pub fn pairs(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim(), v.trim()))
}

fn rgb(s: &str) -> Option<u32> {
    u32::from_str_radix(s.trim(), 16).ok().filter(|&c| c <= 0xFF_FFFF)
}

fn rgb_list(v: &str) -> Vec<u32> {
    v.split(',').filter_map(rgb).collect()
}

fn hex_list(colors: &[u32]) -> String {
    colors.iter().map(|c| format!("{c:06X}")).collect::<Vec<_>>().join(",")
}

/// `"100100"` -> `[0x10, 0x01, 0x00]`.
fn triple(v: &str) -> Option<[u8; 3]> {
    let n = u32::from_str_radix(v.trim(), 16).ok().filter(|&n| n <= 0xFF_FFFF)?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

fn triple_hex(t: [u8; 3]) -> String {
    format!("{:02X}{:02X}{:02X}", t[0], t[1], t[2])
}

/// `"1B1C:1B80"` -> `(0x1B1C, 0x1B80)`.
fn usb_ids(v: &str) -> Option<(u16, u16)> {
    let (vid, pid) = v.split_once(':')?;
    Some((u16::from_str_radix(vid, 16).ok()?, u16::from_str_radix(pid, 16).ok()?))
}

fn nums(v: &str) -> Vec<u32> {
    v.split(',').filter_map(|x| x.trim().parse().ok()).collect()
}

/// Applies the keys present in `text` on top of `c`; unknown or bad values
/// are skipped so one broken line never loses the rest.
fn parse_config(text: &str, mut c: AppConfig) -> AppConfig {
    for (k, v) in pairs(text) {
        let n = v.parse::<u32>().ok();
        match (k, n) {
            ("stages", _) => {
                if let Ok(s) = nums(v).into_iter().map(|x| x as usize).collect::<Vec<_>>().try_into() {
                    c.dpi.stages = s;
                }
            }
            ("enabled", Some(x)) => c.dpi.enabled = x as u8,
            ("active", Some(x)) => c.dpi.active = x as usize,
            ("polling", Some(x)) => c.polling = PollingRate::ALL.into_iter().find(|r| r.hz() == x).unwrap_or(c.polling),
            ("lighting", _) => {
                if let [e, b, s, d] = nums(v)[..] {
                    c.lighting.effect = Effect::from_id(e as u8).unwrap_or(c.lighting.effect);
                    c.lighting.brightness = b as u8;
                    c.lighting.speed = s as u8;
                    c.lighting.direction = if d == 1 { Direction::Reverse } else { Direction::Forward };
                }
            }
            ("steady", _) => c.lighting.steady = rgb(v).unwrap_or(c.lighting.steady),
            ("breathing", _) => c.lighting.breathing = rgb_list(v),
            ("colorful", _) => {
                if let Ok(a) = rgb_list(v).try_into() {
                    c.lighting.colorful = a;
                }
            }
            ("dpi_colors", _) => {
                if let Ok(a) = rgb_list(v).try_into() {
                    c.dpi_colors = a;
                }
            }
            ("debounce", Some(x)) => c.debounce = x as u8,
            ("lod", Some(x)) => c.lod = if x == 1 { Lod::Mm1 } else { Lod::Mm2 },
            ("sleep", Some(x)) => c.power.sleep_min = x as u8,
            ("dark_moving", _) => c.power.dark_while_moving = v == "1",
            ("buttons", _) => {
                for (i, t) in v.split(',').take(BUTTON_COUNT).enumerate() {
                    if let Some(a) = triple(t).and_then(Action::from_triple) {
                        c.buttons[i] = a;
                    }
                }
            }
            (k, _) if k.starts_with("macro") => {
                let Some(i) = k[5..].parse::<usize>().ok().filter(|&i| i < MACRO_COUNT) else {
                    continue;
                };
                c.macros[i] = v.split(',').filter_map(triple).filter_map(MacroEvent::decode).take(MACRO_MAX_EVENTS).collect();
            }
            // Ids of the flat catalogue are renamed to brand/model ids.
            ("device", _) if !v.is_empty() => c.device = Some(crate::devices::migrate_id(v).to_string()),
            ("device_usb", _) => c.device_usb = usb_ids(v),
            ("stage", Some(x)) if x > 0 => c.last_stage = Some(x as u8),
            ("setup_asked", _) => c.setup_asked = v == "1",
            ("theme", _) => c.theme = theme::Choice::from_id(v),
            ("lang", _) => c.lang = Lang::from_code(v),
            ("fx_texture", _) => c.fx_texture = v != "0",
            ("fx_motion", _) => c.fx_motion = v != "0",
            ("tui", _) => c.tui = v == "1",
            ("update_check", _) => c.update_check = v != "0",
            ("update_checked", Some(_)) => c.update_checked = v.parse().unwrap_or(0),
            ("update_skip", _) => c.update_skip = Some(v.to_string()).filter(|v| !v.is_empty()),
            ("update_notified", _) => c.update_notified = Some(v.to_string()).filter(|v| !v.is_empty()),
            ("ignored_mice", _) => c.ignored_mice = v.split(',').map(str::trim).filter(|k| !k.is_empty()).map(String::from).collect(),
            _ => {}
        }
    }
    c
}

fn format_config(c: &AppConfig) -> String {
    let stages: Vec<String> = c.dpi.stages.iter().map(|s| s.to_string()).collect();
    let l = &c.lighting;
    let lines = [
        format!("stages={}", stages.join(",")),
        format!("enabled={}", c.dpi.enabled),
        format!("active={}", c.dpi.active),
        format!("polling={}", c.polling.hz()),
        format!("lighting={},{},{},{}", l.effect.id(), l.brightness, l.speed, l.direction as u8),
        format!("steady={:06X}", l.steady),
        format!("breathing={}", hex_list(&l.breathing)),
        format!("colorful={}", hex_list(&l.colorful)),
        format!("dpi_colors={}", hex_list(&c.dpi_colors)),
        format!("debounce={}", c.debounce),
        format!("lod={}", c.lod as u8),
        format!("sleep={}", c.power.sleep_min),
        format!("dark_moving={}", c.power.dark_while_moving as u8),
        format!("buttons={}", c.buttons.iter().map(|a| triple_hex(a.to_stored())).collect::<Vec<_>>().join(",")),
        format!("device={}", c.device.as_deref().unwrap_or("")),
        format!("device_usb={}", c.device_usb.map(|(v, p)| format!("{v:04X}:{p:04X}")).unwrap_or_default()),
        format!("stage={}", c.last_stage.unwrap_or(0)),
        format!("setup_asked={}", c.setup_asked as u8),
        format!("theme={}", c.theme.id()),
        format!("lang={}", c.lang.map(|l| l.code()).unwrap_or("system")),
        format!("ignored_mice={}", c.ignored_mice.join(",")),
        format!("fx_texture={}", c.fx_texture as u8),
        format!("fx_motion={}", c.fx_motion as u8),
        format!("tui={}", c.tui as u8),
        format!("update_check={}", c.update_check as u8),
        format!("update_checked={}", c.update_checked),
        format!("update_skip={}", c.update_skip.as_deref().unwrap_or("")),
        format!("update_notified={}", c.update_notified.as_deref().unwrap_or("")),
    ];
    let mut text = lines.join("\n") + "\n";
    for (i, m) in c.macros.iter().enumerate().filter(|(_, m)| !m.is_empty()) {
        let events: Vec<String> = m.iter().map(|e| triple_hex(e.encode())).collect();
        text += &format!("macro{i}={}\n", events.join(","));
    }
    text
}

pub fn load_config() -> AppConfig {
    let base = import_vendor().unwrap_or_default();
    let cfg = match load_text(CONFIG_FILE) {
        Some(text) => parse_config(&text, base),
        None => base,
    };
    sanitize(cfg)
}

pub fn save_config(c: &AppConfig) {
    save_text(CONFIG_FILE, &format_config(c));
}

/// Debounce choices offered in the UI, in milliseconds.
pub const DEBOUNCE_CHOICES: [u8; 6] = [4, 6, 8, 12, 16, 20];
/// Sleep timer choices in minutes (vendor UI offers 3–10).
pub const SLEEP_CHOICES: [u8; 8] = [3, 4, 5, 6, 7, 8, 9, 10];

/// Keeps a hand-edited or imported config inside valid bounds.
fn sanitize(mut c: AppConfig) -> AppConfig {
    let all_stages = (1u8 << STAGE_COUNT) - 1;
    c.dpi.enabled &= all_stages;
    if c.dpi.enabled == 0 {
        c.dpi.enabled = all_stages;
    }
    c.dpi.active = c.dpi.active.min(STAGE_COUNT - 1);
    for s in c.dpi.stages.iter_mut() {
        *s = (*s).min(crate::protocol::dpi_steps() - 1);
    }

    let l = &mut c.lighting;
    l.brightness = l.brightness.clamp(1, crate::protocol::BRIGHTNESS_MAX);
    l.speed = l.speed.clamp(1, crate::protocol::SPEED_MAX);
    l.breathing.truncate(BREATHING_MAX);
    if !DEBOUNCE_CHOICES.contains(&c.debounce) {
        c.debounce = DEBOUNCE_DEFAULT;
    }
    if !SLEEP_CHOICES.contains(&c.power.sleep_min) {
        c.power.sleep_min = SLEEP_DEFAULT;
    }

    // Buttons and macros go to the mouse as-is: keep them safe to replay,
    // and never leave the user without a left click.
    c.buttons = c.buttons.map(Action::sanitized);
    if !c.buttons.contains(&Action::Mouse(crate::protocol::MB_LEFT)) {
        c.buttons[0] = DEFAULT_ACTIONS[0];
    }
    c.macros = std::mem::take(&mut c.macros).into_iter().map(crate::protocol::balance).collect();
    c
}

// ---- vendor import -------------------------------------------------------

/// Reads a UTF-16LE ini file (vendor format) into `key=value` pairs.
fn read_utf16_ini(path: &std::path::Path) -> Option<Vec<(String, String)>> {
    let bytes = std::fs::read(path).ok()?;
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let text = String::from_utf16_lossy(&units);

    let pairs = text
        .lines()
        .filter_map(|l| l.trim().trim_start_matches('\u{feff}').split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    Some(pairs)
}

fn value<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn hex_of(pairs: &[(String, String)], key: &str) -> Option<usize> {
    usize::from_str_radix(value(pairs, key)?, 16).ok()
}

fn dec_of(pairs: &[(String, String)], key: &str) -> Option<u8> {
    value(pairs, key)?.parse().ok()
}

/// Vendor per-effect tables: `a|b|c` per effect, effects separated by `;`.
/// `grid[effect][slot]`, e.g. `FF0000|00FF00;0000FF` -> [[FF0000, 00FF00], [0000FF]].
fn grid(v: &str) -> Vec<Vec<&str>> {
    v.split(';').map(|g| g.split('|').collect()).collect()
}

/// Imports the active profile of the vendor app, e.g. `XYSetting1Value=0006`.
fn import_vendor() -> Option<AppConfig> {
    let dir = crate::platform::config_dir()?.join(VENDOR_DIR);
    let main = read_utf16_ini(&dir.join("config.ini"))?;
    let profile = value(&main, "CurrentProfile")?;
    if !profile.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let psf = read_utf16_ini(&dir.join(format!("default{profile}.psf")))?;

    let mut cfg = AppConfig::default();
    for i in 0..STAGE_COUNT {
        cfg.dpi.stages[i] = hex_of(&psf, &format!("XYSetting{}Value", i + 1))?;
    }
    cfg.dpi.enabled = hex_of(&psf, "DPIOffOnFlagValue")? as u8;
    cfg.dpi.active = hex_of(&psf, "CurrentDPILevelValue")?;

    // "030000": first byte is the rate index 0..3 (125/250/500/1000 Hz).
    let rate = hex_of(&psf, "PollingRateValue")? >> 16;
    cfg.polling = *PollingRate::ALL.get(rate)?;

    // Optional parts: keep defaults when a key is missing.
    for (i, c) in cfg.dpi_colors.iter_mut().enumerate() {
        if let Some(v) = value(&psf, &format!("DPILevelIndication{i}Valuer")).and_then(rgb) {
            *c = v;
        }
    }
    cfg.debounce = dec_of(&psf, "KeyDebounce").unwrap_or(cfg.debounce);
    cfg.lod = if dec_of(&psf, "LiftDetectionHeight") == Some(1) { Lod::Mm1 } else { Lod::Mm2 };
    cfg.power.sleep_min = dec_of(&psf, "PowerDownTime").unwrap_or(cfg.power.sleep_min);
    if let Some(m) = dec_of(&psf, "MoveLightingOnOffMode") {
        cfg.power.dark_while_moving = m == 1;
    }
    import_lighting(&psf, &mut cfg.lighting);
    import_buttons(&psf, &mut cfg.buttons);
    Some(cfg)
}

/// Vendor `KeyNValue=100100`. The vendor numbers Forward as Key3 and Back as
/// Key4, while the device has Back at 3 and Forward at 4. Macros are not
/// imported (vendor stores them in another format); such buttons keep defaults.
fn import_buttons(psf: &[(String, String)], buttons: &mut [Action; BUTTON_COUNT]) {
    const PSF_KEY_FOR_DEVICE: [usize; BUTTON_COUNT] = [0, 1, 2, 4, 3, 5];
    for (dev, &key) in PSF_KEY_FOR_DEVICE.iter().enumerate() {
        let action = value(psf, &format!("Key{key}Value")).and_then(triple).and_then(Action::from_triple);
        if let Some(a) = action.filter(|a| !matches!(a, Action::Macro { .. })) {
            buttons[dev] = a;
        }
    }
}

fn import_lighting(psf: &[(String, String)], l: &mut Lighting) {
    let Some(effect) = value(psf, "Lighting1ModeValue").and_then(|v| u8::from_str_radix(v.get(..2)?, 16).ok()) else {
        return;
    };
    let Some(effect) = Effect::from_id(effect) else {
        return;
    };
    l.effect = effect;
    let e = effect.id() as usize;

    // Per-effect slider indices; the wire value is index + 1.
    let pick = |key: &str| -> Option<u8> {
        let v = value(psf, key)?;
        u8::from_str_radix(v.split('|').nth(e)?, 16).ok()
    };
    l.brightness = pick("Lighting1Brightness").map_or(l.brightness, |i| i.saturating_add(1));
    l.speed = pick("Lighting1Speed").map_or(l.speed, |i| i.saturating_add(1));
    l.direction = if pick("Lighting1Direction") == Some(1) { Direction::Reverse } else { Direction::Forward };

    let (Some(colors), Some(selected)) = (value(psf, "Lighting1FixedColor"), value(psf, "Lighting1ColorSelected")) else {
        return;
    };
    let (colors, selected) = (grid(colors), grid(selected));
    let color = |eff: Effect, slot: usize| colors.get(eff.id() as usize)?.get(slot).copied().and_then(rgb);
    let chosen = |eff: Effect, slot: usize| selected.get(eff.id() as usize).and_then(|g| g.get(slot)).is_some_and(|s| *s == "01");

    if let Some(c) = color(Effect::Steady, 0) {
        l.steady = c;
    }
    l.breathing = (0..BREATHING_MAX)
        .filter(|&k| chosen(Effect::Breathing, k))
        .filter_map(|k| color(Effect::Breathing, k))
        .filter(|&c| c != 0)
        .collect();
    for k in 0..COLORFUL_SLOTS {
        if let Some(c) = color(Effect::ColorfulSteady, k) {
            l.colorful[k] = c;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip() {
        let mut c = AppConfig::default();
        c.dpi.stages = [6, 12, 22, 30, 51, 58];
        c.polling = PollingRate::Hz1000;
        c.setup_asked = true;
        c.lighting.effect = Effect::Breathing;
        c.lighting.breathing = vec![0xFF0000, 0x0000FF];
        c.debounce = 8;
        c.lod = Lod::Mm1;
        c.power = Power { sleep_min: 3, dark_while_moving: false };
        let back = parse_config(&format_config(&c), AppConfig::default());
        assert_eq!(back.dpi, c.dpi);
        assert_eq!(back.polling, c.polling);
        assert_eq!(back.lighting, c.lighting);
        assert_eq!((back.debounce, back.lod, back.power), (c.debounce, c.lod, c.power));
        assert!(back.setup_asked);
    }

    #[test]
    fn old_device_ids_migrate() {
        let c = parse_config("device=corsair\ndevice_usb=1B1C:1B80\n", AppConfig::default());
        assert_eq!(c.device.as_deref(), Some("corsair/auto"));
        assert_eq!(c.device_usb, Some((0x1B1C, 0x1B80)));
        let c = parse_config("device=whiteshark-lionel\n", AppConfig::default());
        assert_eq!(c.device.as_deref(), Some("whiteshark/lionel"));
        let back = parse_config(&format_config(&c), AppConfig::default());
        assert_eq!((back.device, back.device_usb), (c.device, None));
    }

    #[test]
    fn ignored_mice_roundtrip() {
        let mut c = AppConfig::default();
        c.ignored_mice = vec!["046D:C547".into(), "05AC:024F:AB12".into()];
        let back = parse_config(&format_config(&c), AppConfig::default());
        assert_eq!(back.ignored_mice, c.ignored_mice);
    }

    #[test]
    fn update_settings_roundtrip() {
        let mut c = AppConfig::default();
        assert!(c.update_check);
        c.update_check = false;
        c.update_checked = 1_790_000_000;
        c.update_skip = Some("0.9.1".into());
        let back = parse_config(&format_config(&c), AppConfig::default());
        assert!(!back.update_check);
        assert_eq!(back.update_checked, c.update_checked);
        assert_eq!(back.update_skip, c.update_skip);
        assert_eq!(back.update_notified, None);
    }

    #[test]
    fn buttons_and_macros_roundtrip() {
        use crate::protocol::{Edge, Ev, MacroMode};
        let mut c = AppConfig::default();
        c.buttons[3] = Action::Macro { index: 2, mode: MacroMode::Toggle };
        c.buttons[5] = Action::Key { mods: 0x01, usage: 0x06 };
        c.macros[2] = vec![MacroEvent { ev: Ev::Key(0x04, Edge::Down), delay_ms: 30 }];
        let back = parse_config(&format_config(&c), AppConfig::default());
        assert_eq!(back.buttons, c.buttons);
        assert_eq!(back.macros, c.macros);
    }

    #[test]
    fn vendor_buttons_swap_back_forward() {
        let psf: Vec<(String, String)> = [("Key3Value", "101000"), ("Key4Value", "100800")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let mut b = DEFAULT_ACTIONS;
        import_buttons(&psf, &mut b);
        assert_eq!(b, DEFAULT_ACTIONS);
    }

    #[test]
    fn config_cannot_remove_left_click() {
        let c = parse_config("buttons=000000,100200,100400,100800,101000,400100", AppConfig::default());
        let c = sanitize(c);
        assert!(c.buttons.contains(&Action::Mouse(crate::protocol::MB_LEFT)));
    }

    #[test]
    fn missing_keys_keep_base() {
        let mut base = AppConfig::default();
        base.debounce = 16;
        let c = parse_config("polling=1000\nbroken line\nlighting=x", base);
        assert_eq!(c.debounce, 16);
        assert_eq!(c.polling, PollingRate::Hz1000);
    }

    #[test]
    fn vendor_grid_parsing() {
        let g = grid("FF0000|00FF00;0000FF");
        assert_eq!(g, vec![vec!["FF0000", "00FF00"], vec!["0000FF"]]);
    }
}
