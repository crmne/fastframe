# fastframe-i18n

Bundled gettext catalogs for egui apps.

Translations live in `assets/i18n/*.po`. The app's build script compiles
each catalog into a Rust module, so the binary needs no libintl, parses no PO
file at run time, and makes no network request. English is the source
language and the fallback for every message a catalog has not translated.

## Build time

Take the crate twice: once for the lookups, once with the `build` feature
as a build-dependency (so the PO parser never reaches the binary).

```toml
[dependencies]
fastframe-i18n = { git = "https://github.com/crmne/fastframe", rev = "<commit>" }

[build-dependencies]
fastframe-i18n = { git = "https://github.com/crmne/fastframe", rev = "<commit>", features = ["build"] }
```

```rust
// build.rs
fn main() {
    fastframe_i18n::build::compile_catalogs("assets/i18n");
    // ... the app's other build steps (Windows resources and so on) ...
}
```

`compile_catalogs` compiles every `<tag>.po` into `$OUT_DIR/<tag>.rs`, a
module named after the tag in lowercase with `_` for `-` (`pt-BR.po` becomes
`pt_br`), and writes the index `$OUT_DIR/catalogs.rs`. It asks Cargo to rerun
when the directory changes and fails the build with the file name on any
error. Like `msgfmt`, it leaves out fuzzy and unfinished entries; a plural
with any empty form is left out whole, so every count shows a complete phrase.

`compile(source, destination)` compiles a single file, for fixtures.

## Run time

The app keeps its own `Locale` enum (which languages it ships, their tags,
native names, and how system tags map onto them) and names each catalog:

```rust
include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));

#[derive(Clone, Copy, Default)]
pub enum Locale { #[default] English, German, PortugueseBrazil }

impl fastframe_i18n::Locale for Locale {
    fn catalog(self) -> Option<&'static dyn fastframe_i18n::Translator> {
        match self {
            Self::English => None,
            Self::German => Some(&de::Translator),
            Self::PortugueseBrazil => Some(&pt_br::Translator),
        }
    }
}

pub use fastframe_i18n::{gettext, ngettext, pgettext};

let label = gettext(locale, "Settings");
let menu = pgettext(locale, "verb", "Follow");
let members = ngettext(locale, "{count} member", "{count} members", count)
    .replace("{count}", &count.to_string());
```

The generated modules implement `fastframe_i18n::Translator` (the `tr`
crate's trait, re-exported), so the app needs no direct `tr` dependency.

### Following the system language

```rust
fn from_tag(tag: &fastframe_i18n::LanguageTag) -> Option<Locale> {
    match tag.language.as_str() {
        "de" => Some(Locale::German),
        "pt" => Some(Locale::PortugueseBrazil),
        "en" => Some(Locale::English),
        _ => None,
    }
}

pub fn detect() -> Locale {
    // Tests assert English strings whatever the machine reads.
    if cfg!(test) {
        return Locale::English;
    }
    fastframe_i18n::detect(from_tag).unwrap_or_default()
}
```

`LanguageTag::parse` reads BCP 47 (`zh-Hant-TW`, `es-419`) and POSIX names
(`pt_PT.UTF-8@euro`) into lowercase `language`, `script` and `region`.
`detect` walks the system's preferred languages (read once per process) and
returns the first one the app supports; `first_supported` does the same over
any list, for tests.

## Updating translations

`scripts/update-translations.sh` extracts messages with `xgettext` into the
template, merges it into every catalog with `msgmerge`, and checks each with
`msgfmt`. It needs GNU gettext with Rust support; normal builds do not. Run
it from the app's root; `assets/i18n/POTFILES` lists the sources to scan.

```sh
update-translations.sh --package ZapFast --domain zapfast \
    --bugs 'https://github.com/crmne/zapfast/issues/new?template=translation.yml' \
    --keyword translated:2
update-translations.sh ... --check   # CI: fail if the template is stale
```

The keywords `gettext:2`, `pgettext:2c,3` and `ngettext:2,3` match this
crate's functions; `--keyword` adds an app's own. `msgmerge` runs without
fuzzy matching unless `--fuzzy-matching` is given: the build drops fuzzy
entries anyway, and an empty message shows the English source.

An app keeps a two-line wrapper that finds the script in the fastframe
checkout Cargo already has:

```sh
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
crate=$(cargo metadata --format-version 1 --locked |
    grep -o '"manifest_path":"[^"]*fastframe-i18n/Cargo.toml"' | head -n1 |
    sed 's/^"manifest_path":"//; s/Cargo.toml"$//')
exec "$crate/scripts/update-translations.sh" --package ZapFast --domain zapfast \
    --bugs 'https://github.com/crmne/zapfast/issues/new?template=translation.yml' \
    --keyword translated:2 "$@"
```
