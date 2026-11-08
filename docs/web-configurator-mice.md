# Web-configurator mice (ATK, VXE, VGN, Lamzu, Pulsar, …)

Research for `src/drivers/oemhid.rs`. Nothing here was tested on hardware by
us; "verified" means verified by the cited project on real hardware.

## Main source

[OpenMouse-Project/mouse-protocol](https://github.com/OpenMouse-Project/mouse-protocol)
(AGPL-3.0 — used as information only, no code copied). It reimplements most
of these web configurators and keeps sanitized wire captures:

- `src/atk/index.ts`, `src/drivers/atk/hid.ts`, `docs/atk-testing.md`,
  `docs/vxe-r1-pro-max-testing.md`, `captures/atk-x1-pro-max/*.json`
- `src/vgn/index.ts`, `src/drivers/vgn/hid.ts`
- `src/drivers/lamzu-atlantis/hid.ts`, `docs/lamzu-atlantis-testing.md`
- `src/pulsar/index.ts`, `src/drivers/pulsar/*.ts`, `docs/pulsar-x2a-v3-testing.md`
- `docs/attackshark-x11-testing.md`

Others: [LeadSun/lamzu-cfg](https://github.com/LeadSun/lamzu-cfg) (Lamzu
Atlantis rate table), the proposed Linux
[hid-pulsar patch](https://lkml.iu.edu/2604.0/01827.html) (battery only),
ATK HUB ([hub.atk.pro](https://hub.atk.pro/), bundle 3.2.27 cited by OpenMouse).

## The shared protocol ("CompX report 8")

Output report 8 → input report 8, 16-byte payload, on collection
`FF02:0002` (interface 1). Not feature reports.

```text
 08 | cmd | status | addr hi | addr lo | len | data[10] | sum
 sum = 0x55 - (report id + bytes 1..15)   (whole report sums to 0x55)
 cmd 04 battery: data = percent, charging, mV (BE)  (len may say 2)
     07 write EEPROM, 08 read EEPROM (≤ 10 bytes), 10 CID/MID, 12 firmware (BCD)
     01 handshake (random challenge), 03 online, 0e/0f profile
```

EEPROM fields are `value, 0x55 - value` pairs: `0x00` rate, `0x02` stage
count, `0x04` active stage, `0x0A` lift-off, `0x0C + 4n` DPI stage n
(`x, y, mode, check`; mode = y nibble << 4 | x nibble), `0xA9..` debounce,
motion sync, sleep, angle snap, ripple.

Rate codes: `08`=125, `04`=250, `02`=500, `01`=1000, `10`=2000, `20`=4000,
`40`=8000 (Lamzu cable also reads `10` as 1000).

DPI encodings differ per sensor — the main trap:

| Codec | Value | Used by |
| --- | --- | --- |
| Step50 | (count+1)·50, count high bits in mode bits 2-3, bit 0 doubles (>30k) | ATK PAW3950/PAW3395, VXE R1 Pro Max, VGN, Lamzu Atlantis, Pulsar on 0x3554 |
| Ten | (count+1)·10 ≤ 10k; bit 1: 10050 + count·50; bit 0 doubles | ATK PAW3950 Ultra (F1 Ultimate), Pulsar on 0x3710 |
| PAW3395SE table | lookup, 35 codes skipped | VXE R1 SE / SE+ |
| PAW3955 Master | 6-byte rows at `0x1B00` | ATK A9 Mini + |

ATK receivers are shared by models, so the model comes from CID/MID
(command `0x10`), not the PID. A wrong codec reads 800 DPI as 160.

## Per brand

| Brand / model | VID:PID | Status in Whisker | Confidence |
| --- | --- | --- | --- |
| ATK X1 Pro Max (CID/MID 2,39) | 373B:1017 cable, 373B:101B 8K, 373B:101A 1K | read + write | High (wire captures, write/readback verified) |
| ATK A9 Plus Nearlink (2,83) | 373B:1115 cable, 373B:10C9 NK dongle | read + write, ≤ 1000 Hz | High |
| ATK F1 Ultimate 2.0 (1,8) | 373B:11D9 8K dongle-L | read + write | Medium (captured from ATK HUB; Ten codec) |
| Other ATK on these PIDs | same | rate + battery only, no DPI | — |
| VXE R1 Pro Max (2,27) | 3554:F58C cable | read + write, ≤ 1000 Hz | High |
| VXE R1 Pro Max | 3554:F58A 1K receiver | read only (writes need a 10-byte system row) | Medium |
| VXE R1 / R1 SE / SE+ | 373B:1085, 3554:F58E, 3554:F58F | documented only: PAW3395SE table, rate in live row `0x0070` (selector `0B`), EEPROM writes only on cable | Medium |
| VGN Dragonfly F2 Master+ | 3554:FB57 cable, 3554:FB56 receiver | read only (no hardware write record found) | Medium |
| VGN Dragonfly F1 | unknown | unknown — PIDs not found | Low |
| Lamzu Atlantis / Mini / Pro / Thorn / Maya / Paro (one id, model not on the wire) | 3554:F50F cable | read + write, ≤ 1000 Hz | High |
| Lamzu same family | 3554:F50D 1K receiver | read only (unverified) | Medium |
| Lamzu same family | 3554:F510 4K receiver | read only (rate change re-enumerates the receiver) | Medium |
| Lamzu Inca / older | 373E:*, 37B0:* | not covered (other protocol) | — |
| Pulsar X2 / Xlite (classic) | 3710:* | documented only: same frames, but needs `03 01` host-control enter/exit around every operation and a `01` handshake for the rate ceiling; PIDs not pinned | Medium |
| Pulsar on CompX | 3554:F507 cable, F508 1K, F509 4K (kernel patch) | documented only (Step50, same handshake) | Medium |
| Pulsar X2 v3 / XS1 family | 3710:3404/3409/3410/5402/5403 | not covered: 64-byte report 0, other protocol | — |
| Pulsar 8K Pro dongle | 3710:5405 | not covered: report 0, 63 bytes, cmds `B1`.. | — |
| Attack Shark X11 | 1D57:FA60 receiver, 1D57:FA55 cable | same receiver as Eternico AET-MS480B already in Whisker (listen-only `03 <model> 40 01 <pct>`) | Medium |
| Attack Shark R-series | 373E:*, 25A7:* | not covered (several unrelated OEM protocols) | Low |
| Darmoshark | unknown | unknown — no open reimplementation found | — |

## Driver rules

- `open()` sends nothing; CID/MID is asked lazily (3 tries).
- Writes only from `set_dpi` / `set_rate`, only where the model and the
  transport are both marked writable, and always confirmed by reading back.
- A mouse moved from an 8K to a 1K receiver keeps its 8K code; the reading is
  capped at the transport's ceiling.
