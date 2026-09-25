# fastframe-icons

Embedded SVG icon sets for egui, and the Lucide icons the apps share.

An app lists its icons once. The `icons!` macro generates the `Icon` enum,
each icon's URI and SVG bytes, and an `image` helper; `install` serves them
to egui.

```rust
fastframe_icons::icons! {
    /// Every icon the interface draws.
    pub enum Icon {
        prefix: "zapfast-icon-",
        directory: "../assets/icons/",
        Archive => "archive",       // embeds ../assets/icons/archive.svg
        Check => lucide "check",    // a shared icon from this crate
    }
}

// Once per egui context, after egui_extras::install_image_loaders (svg feature):
fastframe_icons::install::<Icon>(ctx);

ui.add(Icon::Archive.image(palette.secondary, 18.0));
```

- `directory` is relative to the file that invokes the macro, as with
  `include_bytes!`.
- The enum derives `Clone, Copy, Debug, Eq, PartialEq, Hash`; other
  attributes and doc comments on the enum and its variants pass through.
- `Icon::ALL`, `Icon::uri` and `Icon::bytes` are `const`. URIs are
  `bytes://<prefix><file>.svg`, or `bytes://<prefix>lucide-<name>.svg` for a
  shared icon; keep the prefix unique to the app.
- Buttons, hover tints and sizes stay in the app.

## The loader

`install` adds a bytes loader that never forgets. egui's own
`include_bytes` path forgets an image's bytes once its texture is uploaded
when `reduce_texture_memory` is on; an icon drawn at a second size then finds
neither bytes nor texture after egui prunes the extra size, and paints the
red "failed" placeholder. ZapFast found and fixed this; the loader is its
fix. Several icon sets can be installed side by side.

## Shared icons

`icons/lucide/` holds the 43 Lucide icons that ZapFast and Spotifast ship
byte for byte alike (24 px outlines, drawn in white so egui's tint colours
them). Name them with `lucide "name"`; the lookup runs at compile time, so a
misspelt name fails the build and only the icons an app names reach its
binary. `fastframe_icons::lucide::ALL` lists them.

An icon moves here only when two apps ship the same file. Icons one app
customised, or that differ between apps (RekordFlash's minified
`lucide-static` files, TonePush's own set), stay in the app.

## Licence

The code is MIT. The shared icons are [Lucide](https://lucide.dev) (ISC),
some derived from Feather (MIT); both notices are in
[`icons/lucide/LICENSE.txt`](icons/lucide/LICENSE.txt). An app that embeds
shared icons ships that notice with its other licences, as it already does
for its own Lucide files.
