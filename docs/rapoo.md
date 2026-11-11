# Rapoo Gen 2 mice (A Hub, hub.rapoo.cn)

Research for `src/drivers/rapoo.rs`. "Measured" below means read off a
connected and awake Rapoo VT1 Gen 2 receiver (`24AE:1415`) with the tests at the
end of this file; everything else comes from the sources cited and is marked as
such.

## Sources

- [Nuitfanee/ClickSync](https://github.com/Nuitfanee/ClickSync), GPL-2.0
  (used as information only, no code copied): `src/protocols/protocol_api_rapoo.js`
  is a WebHID driver built against Rapoo's own configurator at
  [hub.rapoo.cn](https://hub.rapoo.cn/); it records the frame format, the
  register map, the rate codes, the DPI encoding and where the battery sits in a
  status report. Every register it names that was asked here answered as it says.
- Issue #3, a Whisker mouse report of the Rapoo VT1 Gen 2: the receiver
  `24AE:1415` ("Rapoo Gaming Device", serial `2607-54L`, HID version 261).
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
  answer  01 00 00 00 | data[0..len]                     feature report 08
  status  07 | 10 01 | dpi (2 B LE) | dpi (2 B LE) | stage | percent | ..
```

Byte for byte these are the frames of Rapoo's own configurator: `A5 A5 len addr
bank 00 00` then the data for a write, `A5 A4 len addr bank 00 00 00` for a read.

`len` is the **length of the answer wanted**, not the length of any data the
frame carries: asking for the table of six DPI stages sets it to 12, and a read
that asks for nothing answers with nothing.

A read of one byte answers with four - the register and the three behind it -
and neighbours agree: `0x96` answered `01 00 01 00` and `0x98` answered
`01 00 02 01`, both saying `0x98 = 01` and `0x99 = 00`. A table read answers
with the table that was asked for. A read that asks for more than the register
holds - 25 bytes at `0x80` - is not answered at all.

An unanswered read leaves the request itself sitting in feature report 08, `00
A5 A4 len` and zeros, so a value is taken only once something behind the four
header bytes is non-zero, and a read that went unanswered is asked again. Windows
hands a feature report out without its id, so the `01 00 00 00` of an answer is
what a `GetFeature` returns first.

The mouse sleeps after two minutes idle (`0xC2` = 02) and answers nothing while
it sleeps. Every probe that only saw zeros behind the header saw a sleeping
mouse, not a wrong frame - the frames were right all along.

Battery comes with a status broadcast the receiver sends on its own, every eight
to ten seconds while the mouse is awake, and cannot be requested. A whole one as
measured, with the report id in front:

```text
07 10 01 c2 01 c2 01 01 5b 00 03 82 00 15 46 01 00 03 c2
       ^ 450 dpi   ^ 450    ^ 1  ^ 91 % ^ 3 ^ 4000 Hz      ^ 194
```

ClickSync reads the percent at the eighth payload byte - the ninth byte of the
report as Windows hands it out with the id in front - and expects a `20 00` head
where this mouse sends `10 01`, while the DPI, the stage and the percent are at
the same places in both.

## Registers (bank 08)

What the connected mouse answered, next to what ClickSync says the register is.

| Address | Content | Answered |
|---|---|---|
| `0x80` | report rate: `08`=125, `04`=250, `02`=500, `01`=1000, `84`=2000, `82`=4000, `81`=8000 | `82` = 4000 Hz |
| `0x81` | key scan rate: `03`=1000, `04`=2000, `05`=4000, `06`=8000 | `03` |
| `0x84` | lift-off height level | `05` |
| `0x85` | motion sync | `01` |
| `0x88..0x93` | six DPI stages, 16 bits little endian | 1200, 450, 800, 800, 800, 800 |
| `0x94/0x95` | unknown | `800` |
| `0x96` | DPI stages in use, written as `count - 1` | `01` = two stages |
| `0x98` | stage in use | `01` = the second one, 450 dpi, which is the DPI the status broadcast names |
| `0x9C..0x9F` | unknown | `50 ff 00 ff` |
| `0xA0` | unknown | `07` |
| `0xC0`, `0xC1` | debounce and lift-off delay, in ms | `04`, `04` |
| `0xC2` | minutes of idle before sleep, 2..120 | `02` |
| `0xC3` | ripple and linear correction bits | `03` |
| `0xC4` | angle snap | nothing |
| `0xC8..0xD3` | the same table for the Y axis | the same six values |
| `0xDC..0xE2` | performance mode, one register per rate: `DC`=125, `DD`=250, `DE`=500, `DF`=1000, `E0`=2000, `E1`=4000, `E2`=8000 | `00 00 01 02`, then `03 03 05 00` |

Bank 06 holds one entry per button (`0x04` answered `03 00 04 00`), bank 00 the
radio with pairing, MAC and keys. Whisker reads and writes neither.

## Writes, measured

Each write was put back to what the mouse had before it, and each one was read
back:

| Write | Read back |
|---|---|
| `0xC2` sleep 2 min, then 32, then 2 | `32`, then `2` |
| `0x80` rate 4000, then 8000 | `82`, then `81` (8000) - a later read answered `82` again after it was written back |
| DPI of the stage in use 450, then 950 | 950, with the status broadcast naming 950 while it was set, then 450 again |

The report rate needs its register alone: the mouse ran at 8000 Hz with the
performance registers `0xDC..0xE2` untouched. A DPI write is the X table and the
Y table written with `count * 2` bytes and the stage count and stage written with
them, which is what A Hub and ClickSync do; writing the two stages in use leaves
the padded rest of the table as it was.

## Still open

1. Whether `0x96` is the DPI stage count on every model. This mouse answers `01`
   for two stages and the rest of the table is padded with 800, the padding A
   Hub writes, which fits `count - 1`; the driver reads and writes the table at
   `count * 2` bytes.
2. What the two head bytes of a status broadcast mean - `10 01` here, `20 00` in
   ClickSync. Charging is likely one of their bits; Whisker does not read it.
3. What the last byte of a status broadcast counts: 194 in one run and 197 in a
   later one, and eleven lower in a broadcast eleven seconds after another.
4. Whether a mouse with more than six DPI stages answers `0x94..0x97` as part of
   the table - this one answers `800` there.
5. DPI above 26000 (PAW3950 models advertise more): the register holds 16 bits,
   so 65535 is the ceiling; the slider stops where A Hub stops.
6. Macro mode, key remapping (bank 06), angle snap (`0xC4` answered nothing),
   glass mode and the `sensorMode` strings of A Hub have registers but no known
   encoding.
7. Feature report 09 and the 32, 24 and 513-byte collections are unread: they
   belong to the rest of the firmware. A `set_feature` on report 08 itself is
   taken but never answered, so the answer only arrives through the read frame.
8. Older Rapoo mice (VT9xx, MT7xx, office mice) use other protocols; none of
   this applies to them.

## Check on a connected mouse

```sh
cargo test -- --ignored --nocapture print_rapoo_probe       # what the driver reads
cargo test -- --ignored --nocapture print_rapoo_blocks      # bank 08 register by register
cargo test -- --ignored --nocapture print_rapoo_exchange    # every answer it gets
cargo test -- --ignored --nocapture print_rapoo_collections # what is there
cargo test -- --ignored --nocapture print_rapoo_writes      # the writes, each put back
WHISKER_RAPOO_RATE=4000 cargo test -- --ignored --nocapture print_rapoo_rate
```

`print_rapoo_probe` runs the driver as the app runs it, `print_rapoo_writes` is
the only one that writes and writes every value back after reading it, and
`print_rapoo_rate` sets the rate named in `WHISKER_RAPOO_RATE` (nothing else
writes a fixed value). **Move the mouse while they run**: asleep, it answers
nothing. In the app, *About → Help add your mouse* reads the mouse, runs the
guided checks and opens a prefilled GitHub issue with the collections and a data
block for `data/mice.toml`.
