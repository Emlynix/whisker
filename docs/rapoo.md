# Rapoo Gen 2 mice (A Hub, hub.rapoo.cn)

Research for `src/drivers/rapoo.rs`. Nothing here was tested on hardware by
us; "verified" means verified by the cited project on real mice.

## Sources

- [Nuitfanee/ClickSync](https://github.com/Nuitfanee/ClickSync), GPL-2.0
  (used as information only, no code copied): `src/protocols/protocol_api_rapoo.js`
  is a WebHID driver built against Rapoo's own configurator at
  [hub.rapoo.cn](https://hub.rapoo.cn/); it records the frame format, the
  register map, the rate codes and the DPI encoding.
- Issue #3, a Whisker mouse report of the Rapoo VT1 Gen 2: the receiver
  `24AE:1415` ("Rapoo Gaming Device") carries the collections below.
- The desktop A Hub ships `RapooDevice.dll` / `libRapooDevice.a`, whose strings
  (`ReturnRate`, `chipType`, `sensorMode`, `ReturnRate` per rate) fit the same
  register map.

## Collections of the VT1 Gen 2

| Usage | Input | Output | Feature | Report ids |
|---|---|---|---|---|
| `0x0001:0x0002` (mouse) | 8 | 8 | 0 | 0x01 |
| `0xFF00:0x000E` | 64 | 33 | 33 | 0x03, 0x06 |
| `0xFF00:0x000F` | 0 | 33 | 33 | 0x08 |
| `0xFF00:0x0002` | 19 | 0 | 0 | 0x07 |
| `0xFF00:0x0001` | 64 | 64 | 64 | 0x09, 0x0A |
| `0xFFA0:0x0001` | 64 | 64 | 64 | 0x0B |

`rapoo.rs` uses the first three of the vendor rows: 06 out, 08 for the answer,
07 for the broadcast. The 64-byte collections of `FF00:0001` and `FFA0:0001`
belong to pairing and to the 8K receiver and are not touched.

## Frames

```text
  write   06 | A5 A5 | len | addr | bank | 00 00 | data[0..len]   padded to 33
  read    06 | A5 A4 | len | addr | bank | 00 00 00   then GetFeature on 08
  wake    06 | A5 A3 | 00 ...
  answer  08 | 01 00 00 00 | data[0..len]
  status  07 | 20 00 | dpi (2 B LE) | .. | stage | percent
```

A read is answered by the firmware writing into feature report 08, so the
answer is fetched with `GetFeature` after a short wait. A write is not
acknowledged: the mouse reports what it took in its next status broadcast.
A Hub waits ~12 ms between frames and one exchange at a time.

## Registers (bank 08)

| Address | Content |
|---|---|
| `0x80` | report rate: `08`=125, `04`=250, `02`=500, `01`=1000, `84`=2000, `82`=4000, `81`=8000 |
| `0x88 + 2n` | DPI stage n of the X axis, 16 bits little endian |
| `0xC8 + 2n` | the same table for the Y axis; both are always written |
| `0x96` | number of stages (up to six) |
| `0x98` | active stage |
| `0xDC + 2i` | speed profile of the rate `RATES[i]` |

Bank 06 holds one entry per button, bank 00 the radio (pairing, MAC, keys).

## Open questions before this is "tested"

1. 8000 Hz: `0x81` exists, but ClickSync writes it only through the collections
   of the 8K receiver. `ability()` does not offer 8000 Hz.
2. `0x96` (stage count): A Hub writes it together with the tables after a DPI
   change. Whisker writes the six values only. Check on a mouse whether a
   table written without the count is kept after a power cycle.
3. DPI above 26000 (PAW3950 models advertise more): the register holds 16 bits,
   so 65535 is the ceiling; the slider stops where A Hub stops.
4. Whether a read needs the `A5 A3` wake frame too. Whisker sends it before a
   write only, so probing an unknown mouse writes nothing.
5. The DPI field of the status broadcast is not used for the reading (the
   registers are read instead); ClickSync stopped trusting it for the same
   reason. Check whether broadcasts arrive while the mouse sleeps.
6. Battery scale: 0..100 assumed, values above 100 are clamped.
7. Macro mode, key remapping (bank 06), angle snap, lift-off (`sensorMode`),
   debounce and sleep timeouts have registers but no known encoding.
8. Older Rapoo mice (VT9xx, MT7xx, office mice) use other protocols; none of
   this applies to them.

## Check on a connected mouse

```sh
cargo test -- --ignored --nocapture print_rapoo_probe
```

Probes every Rapoo device found with read requests only and prints what the
registers answered. In the app, *About → Help add your mouse* reads the mouse,
runs the guided checks and opens a prefilled GitHub issue with the
collections, the probe and a data block for `data/mice.toml`.
