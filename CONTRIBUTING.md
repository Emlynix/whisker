# Contributing to Whisker

Thanks for helping. The most valuable contribution needs no code: telling
us whether Whisker works with your mouse.

## Report a mouse

Open **About → Help add your mouse** in the app. Whisker reads what the
mouse reports about itself (name, USB ids, HID collections, driver
replies; no serial numbers), optionally walks you through a few checks,
and opens a prefilled GitHub issue. Add anything else you noticed.
Reports of experimental mice are how they become supported.

## Report a bug

Open an [issue](https://github.com/Emlynix/whisker/issues) with:

- Whisker version (About), operating system and mouse,
- what you did, what you expected, what happened,
- *Settings → Mouse → Copy for support* output, if a mouse is involved.

**Security issues:** do not open a public issue. Use
[private vulnerability reporting](https://github.com/Emlynix/whisker/security/advisories/new).

## Pull requests

1. For anything larger than a fix, open an issue first so we can agree on
   the approach.
2. Sign the [Contributor License Agreement](CLA.md). The CLA Assistant bot
   asks for it on your first pull request; one signature covers all of
   them.
3. Keep the pull request to one topic. `cargo test` and
   `cargo clippy --release` must pass with no new warnings.

### Code style

- Native code only: no web views, no new runtimes. A new dependency needs
  a good reason.
- Layers talk only to the layer below them:
  `view` → `app` (controller) → `mouse` service → `drivers` / `protocol` →
  `hid_link` → `platform`. The UI never touches HID.
- Keep items private unless another module needs them.
- Early returns instead of nested blocks; braces on every `if`.
- Constants or enums instead of magic numbers and strings; enums instead
  of boolean parameters; function names under 30 characters.
- A short comment says *what* a block does and *why*; an ASCII sketch for
  protocols and data flows.
- Bug fixes start with a test that fails without the fix.
- Drivers: write the code yourself. Protocol facts from other projects
  are fine; copying their code is not. Name the source in the module
  comment.

### Commit messages

- Subject: imperative, capitalized, no period, at most 50 characters
  ("Add Razer Viper V3 receiver id").
- Blank line, then a body wrapped at 72 characters that explains what and
  why; the code shows how.

### Translations

All strings live in `src/i18n.rs`, one table per language. Use the formal
form of address where the language has one, and keep texts as short as
the English ones.

## License

By contributing you agree that your contribution is licensed under the
[GPL-3.0-or-later](LICENSE) and under the terms of the [CLA](CLA.md).
