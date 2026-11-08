# White Shark Lionel — HID configuration protocol

Recovered by static analysis of `LIONEL BLACK.exe` (32-bit MFC, base 0x400000).
Nothing was sent to the device while this was written.

Legend: **[V 0xADDR]** = verified in code at that address. **[I]** = inferred
(reason given). **[L]** = confirmed earlier on the real mouse.

---

## 1. Transport and frame

- 33-byte feature report, report ID `0x0C`, `HidD_SetFeature` / `HidD_GetFeature`,
  vendor usage page 0xFF00. [L]
- Frame layout (every writer memsets the 0x400-byte buffer to 0 first). [V 0x431037]

```
 off  0     1     2    3    4     5      6     7 .. 32
     +----+-----+-----+---+-----+------+-----+--------------+
     | 0C | tgt | cmd | 0 | seq | hdr5 | len | data (len B) |
     +----+-----+-----+---+-----+------+-----+--------------+
```

| Byte | Meaning |
|---|---|
| 1 `tgt` | `0x01` always for config writes. `0x00` only in 0x0F/0x20 when sent through the secondary handle (`+0x200`) while the primary (`+0x204`) is also open. [V 0x432f50, 0x433105] Meaning (wired vs. receiver) [I]. |
| 4 `seq` | One global 8-bit counter, post-incremented for every frame of every command, wraps. [V 0x431052 …] |
| 5 `hdr5` | `0x01`, except cmd 0x0E (chunk header, §5). |
| 6 `len` | Number of data bytes that follow. |

Timing: the app sleeps 60 ms after each SetFeature in config writes [V 0x431172 …],
10 ms between SetFeature(0x20) and GetFeature [V 0x432fe3].

Diffing: the app caches the last payload of every command and sends a command only
when its payload changed (and after "Restore" it re-seeds the cache with defaults,
0x42f750). Our implementation may simply always send. [V 0x430ff0, 0x431184 …]

### Command summary

| cmd | len | Purpose | Source |
|---|---|---|---|
| 0x04 | 4 | Button mapping, one button per frame | [V 0x430550] |
| 0x05 | 18 | DPI levels | [L], [V 0x431042] |
| 0x06 | 24 | DPI-indicator LED colors (8 × RGB) | [V 0x4311a1] |
| 0x07 | 1 | Polling interval (ms) | [L], [V 0x43134a] |
| 0x08 | ≤25 | Lighting effect | [V 0x4314cb] |
| 0x09 | 2 | Lift-off distance | [L], [V 0x4313c8] |
| 0x0A | 1 | **Key debounce** (see §6 conflict) | [V 0x431446] |
| 0x0B | 2 | Power-down time + "lighting off while moving" | [V 0x43168b] |
| 0x0C | 1 | Motion wake-up | [V 0x4317e9] |
| 0x0E | chunked | Macro upload | [V 0x42f68a] |
| 0x0F | 1 | Factory reset (data `FF`) | [V 0x432ea6] |
| 0x20 | 0 | Status query, read back with GetFeature | [L], [V 0x432f9b] |

Apply order used by the app (Apply button, 0x41d050):
`0x04 (changed buttons) → 0x05 → 0x06 → 0x07 → 0x09 → 0x0A → 0x08 → 0x0B → 0x0C → 0x0E (changed macros)`.
No "save to flash"/commit frame exists; nothing is sent after the writes. [V 0x41d096–0x41d111]

---

## 2. Lighting — cmd 0x08 [V 0x430b7a–0x430fc9, sent 0x4314cb]

```
frame[5] = 01
frame[6] = len
frame[7] = effect
frame[8] = (brightness << 4) | (speed & 0x0F)
frame[9..] = effect parameters
```

`brightness` and `speed` are stored per effect in the profile as UI slider index
0-based; the wire nibble is **index + 1**. [V 0x430ba2–0x430bba]

| Field | UI | Wire nibble | Source |
|---|---|---|---|
| brightness (high nibble) | slider 0xc94, 4 positions | 1..4 | [V 0x413be8 (`push 4`), 0x4294f4] |
| speed (low nibble) | slider 0xc80, 3 positions | 1..3 | [V 0x414086 (`push 3`), 0x4294d0] |

Speed slider is hidden for effects 0, 3, 6 [V 0x42ca9b]; the byte is still sent.
Whether the firmware accepts larger nibbles is untested.

### Effect IDs and parameters

UI item → effect id table at 0x621343 (`id 10 → 0`) [V 0x46154e, items 0x461090].

| id | Lan_En name (CN) | In UI | len | Params after byte 8 |
|---|---|---|---|---|
| 0 | None (关闭 = Off) | yes | 2 | — |
| 1 | Colorful Streaming (混彩流光) | yes | 3 | `dir` (0/1) |
| 2 | Breathing (呼吸) | yes | 3+3n | `n`, then n × `R G B` (n ≤ 7) |
| 3 | Steady (单色常亮) | yes | 5 | `R G B` |
| 4 | Neon (霓虹) | yes | 2 | — |
| 5 | Snake (跑马) | yes | 2 | — |
| 6 | Colorful Steady (混彩常亮) | yes | 23 | 7 × `R G B`, fixed slots |
| 7 | Flicker (闪烁) | **no** | 8 | 2 × `R G B`, fixed slots |
| 8 | Stars Twinkle (繁星点点) | **no** | 2 | — |
| 9 | Wave (七彩波浪) | yes | 2 | — |

Details:
- `len` counts the effect byte and the brightness/speed byte. [V 0x430ddd–0x430df3]
- Effect 2: only colors whose "selected" flag is set **and** whose value ≠ 0 are
  packed, in slot order; `n` = how many were packed. [V 0x430bf8–0x430ddd]
- Effects 6, 7: slot *i* always at offset `9 + 3i`; unselected slots stay `00 00 00`.
  [V 0x430e41–0x430fb9]
- Effect 1 `dir`: profile `Direction[1]`, set to 1 when radio state bit 0x800 is on,
  else 0. Which way is 0/1 [I, needs test]. [V 0x429502–0x42951b]
- The builder handles effect ids 7 and 8, but the UI never adds those items for this
  model. [V 0x4611aa]
- `Lighting1OffOn`, `Lighting2*` psf keys are never used to build a frame. "Off" is effect 0.

Color byte order: profile stores colors as COLORREF-like dword, low byte first on the
wire and first in the psf hex string. psf `FF0000` → wire `FF 00 00`. [V 0x430c0d,
writer 0x42b502]. That this is **R,G,B** follows from the palette dialog drawing the
same dword with `CreateSolidBrush` (COLORREF = 0x00BBGGRR) [I, 0x44b02e].

Example — Breathing, red+blue, brightness idx 3, speed idx 1:
`0C 01 08 00 seq 01 09 02 42 02 FF 00 00 00 00 FF` + zero padding.

---

## 3. DPI-indicator colors — cmd 0x06 [V 0x4309db, sent 0x4311a1]

```
frame[5] = 01, frame[6] = 0x18
frame[7 + 3*L .. 9 + 3*L] = R G B of DPI level L (L = 0..7)
```
Source: psf `DPILevelIndication{L}Valuer=RRGGBB` (profile dwords +0x484..+0x4a0).
Byte order as in §2. Sent for all 8 levels regardless of `DPIOffOnFlagValue`.

## 3b. DPI — cmd 0x05 (already known; for completeness) [V 0x4307c4–0x4309d9]

```
frame[6] = 0x12
frame[7]  = ((currentLevel + 1) << 4) | popcount(enabledMask)
frame[8]  = enabledMask            (psf DPIOffOnFlagValue, bit L = level L on)
frame[9 + 2L], [10 + 2L] = two bytes from sensor DPI table 0x65e6a8[XYSetting{L+1}*2]
```

## 3c. Other single-value settings

| cmd | data | Source of value | Source |
|---|---|---|---|
| 0x07 | interval ms: PollingRateValue byte0 0→8, 1→4, 2→2, other→1 | psf `PollingRateValue` (popup: 125/250/500/1000 Hz → 0..3) | [V 0x4306ab, 0x461caa] |
| 0x09 | `[7]` = 1 if LiftDetectionHeight == 1 else 2; `[8]` = 0 | `LiftDetectionHeight` (1 mm / 2 mm) | [V 0x430738] |
| 0x0A | `[7]` = KeyDebounce (default 12) | `KeyDebounce` | [V 0x43076a, 0x431469] |
| 0x0B | `[7]` = PowerDownTime (minutes, default 6), `[8]` = MoveLightingOnOffMode (default 1) | power manager page | [V 0x4315b8–0x4316cb] |
| 0x0C | `[7]` = MotionWakeup (default 1) | power manager page | [V 0x431771] |

Profile offsets for these come from the setting reader at 0x43e150 (base = profile+0x138,
name/value CString pairs). psf `AngleSnapValue` / `LiftDetectionValue` (+0x4a4/+0x4a5)
are only loaded/saved, never sent. [V xrefs 0x429c01, 0x42b23a only]

---

## 4. Button remapping — cmd 0x04 [V 0x430140–0x4305e5]

One frame per **changed** button:
```
frame[5] = 01, frame[6] = 04
frame[7] = button index (device order)
frame[8], [9], [10] = action triple (a0 a1 a2)
```

Device button order — the app swaps psf Key3/Key4 before sending [V 0x4302d6–0x430329]:

| idx | psf key | Default triple |
|---|---|---|
| 0 | Key0 左键 Left | `10 01 00` |
| 1 | Key1 右键 Right | `10 02 00` |
| 2 | Key2 中键 Middle | `10 04 00` |
| 3 | **Key4** 后退 Backward | `10 08 00` |
| 4 | **Key3** 前进 Forward | `10 10 00` |
| 5 | Key5 DPI循环 DPI button | `40 01 00` |

The psf `KeyNValue=AABBCC` is the triple in order. [V writer 0x42b783]

### Action triples

Main menu table (id → triple) at 0x62136f/0x62135f/0x62134f [V 0x462fda]; sub-menus as noted.

| a0 | a1 | a2 | Action | Source |
|---|---|---|---|---|
| 00 | 00 | 00 | Disable | [V table id 0x10] |
| 10 | mask | 00 | Mouse button: 01 L, 02 R, 04 M, 08 Back, 10 Fwd | [V table ids 1–5] |
| 30 | interval | loops | Fire key: interval 5..255, loops 0..255 (0 = while held) | [V dialog 0x44fa76] |
| 30 | 32 | 03 | Three Click (preset fire key: 50, 3×) | [V table id 8] |
| 40 | 01 | 00 | DPI loop | [V 0x46535f, tables 0x62138f/0x62138b] |
| 40 | 02 | 00 | DPI + | same |
| 40 | 03 | 00 | DPI − | same |
| 50 | n | 00 | DPI Lock, n = 1..11 ↔ menu 200,300…1200 DPI | [V 0x464952]; DPI value meaning [I] |
| 60 | 04 | 00 | Switch lighting effect | [V table id 0xe] |
| 70 | mods | usage | Keyboard key / combination. mods: 01 Ctrl, 02 Shift, 04 Alt, 08 Win; usage = HID keyboard usage (VK→HID table 0x621238) | [V 0x437f80–0x43803c, 0x4382a0] |
| 80 | lo | hi | Consumer (multimedia) usage, 16-bit LE | [V 0x466543] |
| 90 | (loop<<4)\|slot | count | Macro, see §5 | [V 0x42f31e–0x42f34a] |

Preset sub-menus (all verified from their tables):

| Menu | Item | Triple |
|---|---|---|
| Multimedia [V 0x466543] | Media Select | `80 83 01` |
| | Prev Track | `80 B6 00` |
| | Next Track | `80 B5 00` |
| | Stop | `80 B7 00` |
| | Play/Pause | `80 CD 00` |
| | Mute | `80 E2 00` |
| | Volume + | `80 E9 00` |
| | Volume − | `80 EA 00` |
| | Mail | `80 8A 01` |
| | Calculator | `80 92 01` |
| Office [V 0x467033] | Copy / Paste / Cut | `70 01 06` / `70 01 19` / `70 01 1B` |
| | Select All / New / Save / Print | `70 01 04` / `70 01 11` / `70 01 16` / `70 01 13` |
| | Open | `70 08 08` (Win+E) |
| | Undo | `70 01 1D` |
| Window mgmt [V 0x468ae3] | My Computer | `80 94 01` |
| | Close Window | `70 04 3D` (Alt+F4) |
| | Show Desktop / Run | `70 08 07` / `70 08 15` |
| | Minimize / Maximize | `70 08 51` / `70 08 52` |

Not offered for this model: Switch Polling Rate, Tilt Left/Right, Profile switch (no items).

---

## 5. Macros

### 5.1 Binding [V 0x42eef0]
Before sending, the app walks buttons 0..5 (psf order); every button whose triple
starts with `0x90` gets a **slot** 1..6 (first-come, one slot per distinct macro).
Its triple becomes:
```
a0 = 0x90
a1 = (loopType << 4) | slot
a2 = loopType == 2 ? loopCount : 0
```
| loopType | Meaning | Source |
|---|---|---|
| 1 | Cycle until key release | [I] radio bit 0x2000, 1st of strings 194–196 |
| 4 | Cycle until key press (again) | [I] radio bit 0x4000 |
| 2 | Specified cycle times (`a2` = count) | [V 0x42f331] count only for 2 |

New macros default to type 2, count 1. [V 0x43a1c4]

### 5.2 Upload — cmd 0x0E [V 0x42f370–0x42f749]
For every macro-bound button: build an event stream (3 bytes per event), then split
it into 25-byte chunks:

```
frame[2] = 0E
frame[5] = (chunkIndex << 4) | totalChunks     chunkIndex 0-based
frame[6] = bytesInThisChunk + 1                (26 for a full chunk)
frame[7] = slot
frame[8..32] = 25 bytes of the stream, zero padded
```
`totalChunks = max(1, ceil(3*eventCount / 25))` (4 bits → ≤ 15 chunks, ≤ 125 events).
Events may straddle chunk borders (plain byte stream). 60 ms sleep per chunk.
Sent after the cmd 0x04 frames. Event count itself is not sent.

### 5.3 Event encoding [V 0x42f460–0x42f534]

```
e0 = up<<7 | type | (delay >> 8 & 0x0F)
e1 = delay & 0xFF
e2 = code
```
- `delay` = ms to wait **after** this event, 0..4095 (clamped). It is taken from the
  delay action that follows the event in the editor list; no delay action → 0.
- `up` = 1 for key/button release, 0 for press.

| type | Event | e2 |
|---|---|---|
| 0x10 | Mouse button | mask 01 L, 02 R, 04 M, 08, 10 (VK 0xE0..0xE4) — 08/10 = Back/Fwd [I] |
| 0x20 | Modifier key | bit mask of HID usage E0..E7 (01 LCtrl, 02 LShift, 04 LAlt, 08 LGUI, 10 RCtrl …) |
| 0x30 | Keyboard key | HID keyboard usage (< 0x91) |
| 0x40 | Cursor X move | int8, negative = left |
| 0x50 | Cursor Y move | int8, negative = up |
| 0x60 | Wheel | int8; UI shows negative as "Wheel Up" [V 0x46e4e9] |

Move/wheel magnitudes are 1..127 in the editor. [V 0x448152]

App-side psf `AllAction` format (336 × 4 bytes) for reference: key `VK, 00|80, 00, 00`;
delay `03, d2, d1, d0` (24-bit ms); move `E5/E6/E7, 00, 00, int8`. VK→HID table at 0x621138.

Example — macro in slot 1, "A down, 172 ms, A up, 47 ms", once:
- cmd 0x04 for the button: `… 04 idx 90 21 01`
- cmd 0x0E: `0C 01 0E 00 seq 01 07 01  30 AC 04  B0 2F 04` + padding

---

## 6. Reset / save / misc

- **0x0F `FF`** = restore factory defaults. Sent by the Restore button after the
  confirm dialog (lang 140); the app then reloads the default profile and resets its
  caches (0x42f750). [V 0x4186c8]
- **No save/commit command.** Settings are pushed and the app does nothing else;
  persistence is assumed to be automatic. [I]
- `0x71`, `0x55 0x81`, `0x90`, `0x09` seen in buffers at 0x43019c, 0x43065d,
  0x42f40c, 0x430bd4 are **scratch headers** of change-detection buffers. The buffer
  is memset before each real frame, so they never reach the wire. [V]
- 0x20 status reply (partial, [V 0x433220]): `[2]=20 [3]=00`; `[8..11]="M022"` model
  check; `[0x0F]` sensor/model code (selects DPI table); `[0x12]` battery %;
  `[0x13]` charge state (2 → shown as 100 %); `[0x15]` ≠ 0 = mouse online.
  If `[2]==0` the app re-sends the query.

---

## 7. Profile struct ↔ psf (base 0x9ba000 + idx*0x370)

| Offset | psf key |
|---|---|
| +0x1b4 / +0x1bc / +0x1c4 / +0x1cc / +0x1d4 | MoveLightingOnOffMode / PowerDownTime / LiftDetectionHeight / KeyDebounce / MotionWakeup (CString) |
| +0x1d8 + 16k | Key{k}: +4..+6 triple, +8 MacroKeyIndex, +0xC macro object ptr |
| +0x23c..0x23e | PollingRateValue |
| +0x240 | Lighting1OffOn |
| +0x248..0x24a | Lighting1ModeValue (byte0 = effect) |
| +0x24b[10] / +0x255[10] | Lighting1Speed / Lighting1Brightness (per effect) |
| +0x25f[10][10] | Lighting1ColorSelected |
| +0x2c4[10][10] dword | Lighting1FixedColor |
| +0x454[10] | Lighting1Direction |
| +0x470 / +0x472[8] word / +0x482 | DPIOffOnFlag / XYSetting1..8 / CurrentDPILevel |
| +0x484[8] dword | DPILevelIndication0..7 |
| +0x4a4 / +0x4a5 | AngleSnapValue / LiftDetectionValue (not sent) |

Macro object: +4 index, +8 action count, +0x550 loopType, +0x551 loopCount,
+0x552 event count (non-delay actions).

---

## 8. Open questions / needs a live test

1. **cmd 0x0A**: code sends `KeyDebounce` (default 12), not angle snap. Earlier live
   note says 0x0A = angle snap; re-check what actually changes on the mouse.
2. Brightness/speed: accepted nibble range beyond UI (1..4 / 1..3)? Does nibble 0 mean anything?
3. Effect 1 direction 0 vs 1; whether hidden effects 7 (Flicker) and 8 (Stars) work.
4. Color order R,G,B (strongly inferred) — confirm with a single red frame.
5. Device button idx 3/4 physically = back/forward (inferred from the swap and defaults).
6. Macro loop types 1 vs 4 (release vs. press) — order inferred from radio bit order.
7. Macro mouse-button masks 08/10 = back/forward; wheel sign.
8. DPI Lock `n` → 200 + 100·(n−1) DPI, or index into the sensor table?
9. Fire key interval unit (ms assumed).
10. Meaning of frame byte 1 = 0x00 path (secondary handle) — which interface is which.
11. Is a re-send needed after power cycle (i.e., does the firmware persist)? Read back
    via 0x20 does not expose settings, so verify by replug.
