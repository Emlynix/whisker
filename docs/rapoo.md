# Rapoo Gen 2 mice (A Hub, hub.rapoo.cn)

Research for `src/drivers/rapoo.rs`. "Measured" below means read off the
connected VT1 Gen 2 receiver with `print_rapoo_collections` and
`print_rapoo_exchange`; the rest comes from the sources cited and is marked as
such.

## Sources

- [Nuitfanee/ClickSync](https://github.com/Nuitfanee/ClickSync), GPL-2.0
  (used as information only, no code copied): `src/protocols/protocol_api_rapoo.js`
  is a WebHID driver built against Rapoo's own configurator at
  [hub.rapoo.cn](https://hub.rapoo.cn/); it records the frame format, the
  register map, the rate codes and the DPI encoding.
- Issue #3, a Whisker mouse report of the Rapoo VT1 Gen 2: the receiver
  `24AE:1415` ("Rapoo Gaming Device").
- The desktop A Hub ships `RapooDevice.dll` / `libRapooDevice.a`, whose strings
  (`ReturnRate`, `chipType`, `sensorMode`, `ReturnRate` per rate) fit the same
  register map.

## Collections of the VT1 Gen 2 (measured)

Three USB interfaces: 0 is the mouse, 1 the vendor collections, 2 a 513-byte
one. Report lengths include the report id byte.

| Usage | Input | Output | Feature | Report id |
|---|---|---|---|---|
| `0x0001:0x0002` mouse | 8 | 0 | 0 | `0x01` |
| `0xFF00:0x000E` | 33 | 33 | 0 | `0x06` |
| `0xFF00:0x000F` | 0 | 0 | 33 | `0x08` |
| `0xFF00:0x0010` | 0 | 0 | 33 | `0x09` |
| `0xFF00:0x0011` | 32 | 32 | 0 | `0x11` |
| `0xFF00:0x0012` | 24 | 24 | 0 | `0x12` |
| `0xFF00:0x0002` | 19 | 19 | 0 | `0x07` |
| `0xFF00:0x0001` | 513 | 513 | 0 | none |
| keyboard, system control, consumer control | 2..19 | 0..2 | 0 | `0x02..0x05` |

`rapoo.rs` writes on `0xFF00:000E` (id 06), reads the answer on `0xFF00:000F`
(id 08) and listens on `0xFF00:0002` (id 07). Feature report 09, the 32 and
24-byte collections of usages `0x11`/`0x12` and the 513-byte collection belong
to the rest of the firmware (pairing, the high report rates) and are not
touched. A report has to be written with exactly the length the collection
declares: a 33-byte write on a 64-byte collection is simply refused.

## Frames

```text
  write   06 | A5 A5 | len | addr | bank | 00 00 | data[0..len]
  read    06 | A5 A4 | len | addr | bank | 00 00 00   then GetFeature on 08
  wake    06 | A5 A3 | 00 ...
  answer  00 | A5 A4 | len | data[0..len]      feature report 08
  status  07 | 20 00 | dpi (2 B LE) | .. | stage | percent
```

Measured: a read frame goes out, the receiver takes some hundred milliseconds,
and feature report 08 then holds the frame's `A5 A4` and its length back. What
follows is the register: on a sleeping mouse it is zeros. The values are
therefore read as "no answer" until something behind the four header bytes is
non-zero, and the read is asked for a few times.

`len` in a read frame is the **length of the answer wanted**, not of any data
the frame carries: asking for a DPI table of six stages sets it to 12. A read
that asks for nothing answers with nothing.

Battery comes with a status broadcast the receiver sends on its own — it cannot
be requested. ClickSync reads the percent at the eighth payload byte, which is
the ninth byte of the report as Windows hands it out with the id in front.

## Registers (bank 08)

| Address | Content |
|---|---|
| `0x80` | report rate: `08`=125, `04`=250, `02`=500, `01`=1000, `84`=2000, `82`=4000, `81`=8000 |
| `0x88 + 2n` | DPI stage n of the X axis, 16 bits little endian |
| `0xC8 + 2n` | the same table for the Y axis; both are always written |
| `0x96` | number of stages, written as `count - 1` |
| `0x98` | active stage |
| `0xDC..0xE2` | speed profile, one register per report rate |

Bank 06 holds one entry per button, bank 00 the radio (pairing, MAC, keys).

## Open questions before this is "tested"

1. Whether the mouse has to be awake for a register read to be answered. The
   probe on a sleeping mouse gave the header with zeros behind it, and no status
   broadcast at all.
2. 8000 Hz: `0x81` is the code, and the VT1 Gen 2 does 8K. Whether the rate
   register alone is enough, or whether `0xE2` (the 8K speed profile) and the
   32 and 24-byte collections of usages `0x11`/`0x12` are needed too, is
   untested.
3. Whether a read needs the `A5 A3` wake frame too. Whisker sends it before a
   write only, so probing an unknown mouse writes nothing.
4. DPI above 26000 (PAW3950 models advertise more): the register holds 16 bits,
   so 65535 is the ceiling; the slider stops where A Hub stops.
5. The DPI field of the status broadcast is not used for the reading (the
   registers are read instead); ClickSync stopped trusting it for the same
   reason.
6. Battery scale: 0..100 assumed, values above 100 are clamped.
7. Macro mode, key remapping (bank 06), angle snap, lift-off (`sensorMode`),
   debounce and sleep timeouts have registers but no known encoding.
8. Older Rapoo mice (VT9xx, MT7xx, office mice) use other protocols; none of
   this applies to them.

## Check on a connected mouse

```sh
cargo test -- --ignored --nocapture print_rapoo_probe       # what the driver reads
cargo test -- --ignored --nocapture print_rapoo_exchange    # every answer it gets
cargo test -- --ignored --nocapture print_rapoo_collections # what is there
```

`print_rapoo_probe` runs the driver as the app runs it. `print_rapoo_exchange`
wakes the mouse, asks the rate, the stage count, the active stage and the DPI
table, prints every answer and every report that arrives, and then listens for
two seconds of status broadcasts. **Move the mouse while they run.** In the app,
*About → Help add your mouse* reads the mouse, runs the guided checks and opens
a prefilled GitHub issue with the collections and a data block for
`data/mice.toml`.
