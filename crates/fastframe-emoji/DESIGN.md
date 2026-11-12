# System font and colour emoji: design note

Approved by Carmine on 2026-09-29. ZapFast draws colour emoji today
(`src/emoji.rs`), Spotifast wants them (it draws a monochrome bundled face
now), and both use `fastframe-fonts`. This note covers what moves here and
how each app chooses.

## Two independent choices

An app picks its interface font and its emoji separately. Spotifast keeps
Inter and wants system colour emoji; ZapFast wants both from the system.

| Choice | Crate | Knob |
| --- | --- | --- |
| Interface font | `fastframe-fonts` | `FontSetup::primary(Primary::Inter)` (default) or `Primary::System` |
| Emoji | `fastframe-emoji` (new) | `EmojiSetup::default().system(true).bundled(bytes)` |

No user-facing setting is needed; each app decides in code.

## Crate placement

- **System interface font** goes into `fastframe-fonts`, which already
  decides which fonts an app registers and already asks CoreText, fontconfig
  and the Windows font directories for fallbacks. `Primary::System` puts the
  platform's interface face (San Francisco, Segoe UI, fontconfig's
  `system-ui`/`sans-serif`) where Inter goes, at each registered weight, and
  realigns the script fallbacks to its baseline. It falls back to Inter when
  the face cannot be found or read. Inter moves behind a default `inter`
  feature, so an app that never draws it can leave the 880 KB out.
- **Colour emoji** get a crate of their own, `fastframe-emoji`: a different
  job (pictures, not outlines), its own dependencies (harfrust, png, and on
  Windows the `windows` crate) and its own egui glue. `fastframe-fonts` stays
  free of them.

## Public API

```rust
// Once, at startup (cheap: it only records the choice).
fastframe_emoji::EmojiSetup::default()
    .system(true)                 // the platform's emoji font
    .bundled(NOTO_COLOR_EMOJI)    // the app's own fallback, optional
    .install();
std::thread::spawn(fastframe_emoji::warm_up); // find and map the fonts

// egui glue, as ZapFast uses it today:
fastframe_emoji::append(ui, &mut job, &mut placements, text, &format);
fastframe_emoji::paint(ui, &galley, origin, &placements);
fastframe_emoji::paint_cluster(ui, cluster, rect);
let (job, clusters) = fastframe_emoji::editor_job(text, &format);

// Outside egui (Spotifast's pixel text): a premultiplied RGBA picture.
let picture = fastframe_emoji::get()?.render("👍🏽", 48);
```

Segmentation (`pieces`, `is_emoji`, `only_emoji`) moves with it, unchanged.
The bundled font is the app's bytes, not the crate's: ZapFast already ships
Noto Color Emoji (10 MB) and Spotifast does not want it.

## Per-platform drawing

Every source answers one question: "draw this whole cluster as one picture,
or say no". A cluster is tried whole, then without U+FE0F, then with its
trailing parts dropped (from PR #229), and every source gets each form
before a part is dropped, so the bundled font's exact picture beats the
system font's partial one.

| Platform | System source | How sequences join | Pixels |
| --- | --- | --- | --- |
| Linux | fontconfig's `emoji` match, else a scan of the font directories for a colour bitmap face (Noto Color Emoji, Twemoji, JoyPixels...) | harfrust applies GSUB | CBDT PNGs through skrifa |
| macOS | `/System/Library/Fonts/Apple Color Emoji.ttc`, memory-mapped | harfrust applies AAT `morx` (PR #279) | sbix PNGs through skrifa |
| Windows | Segoe UI Emoji through DirectWrite (PR #229) | DirectWrite | Direct2D into a WIC bitmap (COLR) |
| All | the app's bundled font | harfrust | CBDT through skrifa |

Linux and macOS share one pure-Rust path: shape the cluster with harfrust,
require exactly one advancing glyph (zero-advance layer glyphs are allowed:
Apple builds couples from a layer under a glyph), then composite each glyph's
bitmap into the font's own cell (advance by ascent plus descent) and scale
it to the requested height. harfrust and skrifa are already in every app's
tree through epaint. That path is tested here on Linux with a subset of Noto;
the macOS font needs a Mac.

For macOS this is preferred over CoreText (PR #229): no FFI, no per-emoji
font creation, the font is mapped rather than read (only the touched strike
pages are paged in), and the code is the Linux code. The risk is that
harfrust's `morx` support misses a sequence Apple joins; PR #279 checked
flags, skin tones, families, the rainbow flag and keycaps on macOS 26, and a
miss degrades to the bundled picture, not to a broken one. CoreText stays
the fallback plan if a Mac shows gaps.

Checked on macOS 27 against CoreText: harfrust gives CoreText's glyphs for
all 3,944 RGI sequences and the 108 people facing right, and the pictures
match CoreText's pixel for pixel, with two Apple details handled here.
The people facing right are `sbix` `flip` records (another glyph's bitmap,
mirrored), which skrifa does not read. And CoreText draws every Apple
picture an eighth of an em below the position its data give, so the
pictures are framed that way too.

Windows keeps DirectWrite: Segoe UI Emoji is COLR outlines, which a
pure-Rust painter could only draw by carrying a vector rasterizer with
gradients. The drawing and the "is it one picture" test are PR #229's,
including Windows 11's families built from overlapping part glyphs, and its
lesson that the COM factories must never be released. Segoe has no country
flags, so they come from the bundled font.

## Caching and threads

- Finding and mapping the fonts happens once per process, in `warm_up`,
  which the app runs on a thread of its own at startup (ZapFast already
  does). The 190 MB Apple font and the 10 MB Noto are mapped, never read.
- A cluster's picture is made once per egui context and physical size it is
  painted at, and kept as a texture. Drawing at the size the GPU shows keeps
  small emoji sharp: a fixed large texture shrunk to a 12-pixel row loses
  their edges, and a font whose nearest strike is larger is upscaled and then
  shrunk again. `paint_cluster` never draws a new picture on the interface
  thread: a missing picture is queued to one worker thread, the placeholder
  stays transparent for that frame, and the worker asks for a repaint when
  the picture is ready. Opening the picker with a thousand unseen emoji costs
  the interface thread nothing but texture uploads.
- `prewarm(ctx, clusters)` queues a set ahead of time (the picker's first
  page). Its pictures are drawn at a default size and serve a first frame at
  any size until that frame's own picture is ready.
  `EmojiSetup::synchronous(true)` draws inline, for tests and demo
  screenshots that must show every emoji in their first frame.
- The harfrust shaper data and the fonts are shared by every thread;
  DirectWrite factories live in a thread-local on the worker, never freed.

## Layout, selection and copy

Unchanged from ZapFast: each emoji cluster becomes one transparent
placeholder glyph (U+2B1B), scaled to the painted picture's width and pinned
to the row height, and `placements` lists the clusters in logical order. The
galley's own selection works on the placeholder, and ZapFast's
`transcript::refine` maps each placeholder back to its cluster when copying.
The editor path (`editor_job`) keeps the real text and paints over the
transparent glyphs. The picture is framed in the emoji font's own cell, so
each platform's emoji sit on the text as that platform draws them.

## System interface font

| Platform | Face | Weights |
| --- | --- | --- |
| macOS | CoreText's system UI font (`SFNS.ttf`), mapped | variable `wght`, `opsz` held at the text cut |
| Windows | Segoe UI Variable if installed, else the message font (`Segoe UI`) | variable `wght`, else the nearest static face |
| Linux | `fc-match 'system-ui:weight=N'`, once per weight (fastframe-text already asks `fc-match`) | fontconfig's own answer per weight, `wght` set when the face is variable |

Each face must draw Latin outlines or it is skipped for Inter. Script
fallbacks are realigned to the chosen face's baseline. Faces are found once
per process, like the fallbacks.

Known difference: Inter's figures are frozen tabular; San Francisco and
Segoe UI draw proportional figures by default, so timers that count will
jitter unless the app keeps a tabular face for them.

## Binary size

- Inter is 880 KB (`InterVariable.ttf`). With `default-features = false` on
  `fastframe-fonts` and `Primary::System`, it is not linked; a machine
  without a usable system face then gets egui's own fonts.
- The bundled Noto Color Emoji is 10.2 MB and dominates. It stays the
  app's choice: needed on Windows for flags and anywhere the system lacks
  a sequence. Dropping it on Linux means no colour emoji where none is
  installed.

## Emoji in every egui text: the plugin

Added 2026-09-29 at Carmine's request: colour emoji should come from
fastframe itself, in every widget, without each app laying its text out
through `append` and `paint`. Spotifast drew them with 300 lines of its own
(`src/emoji.rs`), and labels, buttons, menus, tooltips and text fields it did
not route through that code kept the monochrome face.

Colour glyphs inside epaint's text renderer would be the natural home, but
that is an egui change that may never be accepted upstream, and fastframe
does not patch egui. An egui [`Plugin`](https://docs.rs/egui/0.36/egui/trait.Plugin.html)
reaches every text without it: `Plugin::output_hook` sees the frame's
`FullOutput` after every widget has painted and before the backend
tessellates it.

```rust
ctx.add_plugin(fastframe_emoji::EmojiPlugin::default());
```

That is the whole opt-in. An app still chooses its fonts with
`EmojiSetup` and runs `warm_up` on a thread; the plugin only draws.

### Finding the text

The hook walks `FullOutput::shapes`, recursing into `Shape::Vec`, and
looks at every `Shape::Text`. Text already turned into a `Shape::Mesh`
before the hook (an app that tessellates itself, a `Shape::Callback`
painting with its own renderer) is out of reach, as is anything drawn
outside egui (Spotifast's Winamp pixel text and MilkDrop overlay, which
have [`Emoji::render`]). Rotated text (`angle` other than zero) is left
alone.

### Which glyphs are an emoji

`LayoutJob::text` is split with [`pieces`], the same segmentation `append`
uses. Glyphs are matched to characters without epaint's `Glyph::cluster`,
which only the apps' egui fork has: stock epaint emits one glyph per
character (a continuation of a joined sequence is a zero-width glyph with
no texels, carrying the sequence's first character), rows omit their
`\n`, and a row's glyphs sorted by `first_vertex` are in logical order even
after a right-to-left row is reordered (reordering moves glyphs with their
mesh). So the rows' glyphs, taken in that order, pair off with the text's
characters. An elided galley's last glyph is the overflow character and
pairs with nothing. A cluster is coloured only when its first glyph
carries its first character; any other mismatch leaves the whole galley
as egui drew it.

The cluster's rectangle is the union of its glyphs on that row, and the
picture is fitted into it. The monochrome fallback face does not join every
sequence (Noto Emoji draws a family as its people, a keycap as its digit),
so with stock egui that rectangle is several emoji wide, or a digit wide,
and the pictures differ in size. Added 2026-10-02: the apps' egui fork
lays every emoji grapheme cluster out as one glyph carrying a single
emoji's advance (the advance of U+1F600 in the font), with zero-width
continuation glyphs for the rest of the cluster, drawing the first glyph
the font made of it, centred. Every cluster's rectangle is then one emoji
wide, so the pictures match, and cursors, selection, wrapping and widget
sizes agree with what is painted. Repositioning glyphs inside the plugin
was tried first and dropped: egui's widgets measure, hit-test and place
the cursor from their own galley, which the plugin cannot change.

### Hiding the monochrome glyphs

A galley with a cluster to colour is cloned (only its rows that hold one)
and each of that cluster's glyph quads gets transparent vertices; the
text shape then points at the clone. Layout, wrapping, elision, selection
highlights and the cursor are untouched, since they are other vertices or
other shapes. A text shape with `override_text_color` would repaint those
vertices, so its colour is baked into the clone's glyph vertices first and
the override dropped. `opacity_factor` stays on the text and tints the
pictures the same way, so fading text fades its emoji.

Glyphs an app already made transparent are left alone: fastframe's own
`PLACEHOLDER` (U+2B1B) from `append`, and the transparent emoji of
`editor_job`, which the app paints itself. That rule is per cluster, not
per galley, so a typed ⬛ beside them is still coloured.

### Painting

Each picture is a textured rectangle appended right after its text shape,
in the same `Shape::Vec` or at the same place in the list, with the text's
clip rectangle, so layer order and clipping (a scrolled `TextEdit`, a
table cell) are the text's own. It is sized as `paint_cluster` sizes it:
1.08 rows tall, centred, in the emoji font's cell proportions, smaller when
the glyphs' width is narrower. ZapFast's placeholders and the plugin's
pictures therefore match side by side.

A picture that is not drawn yet keeps the monochrome glyph for that frame
and is queued to the worker, which asks for a repaint; a cluster no font
draws keeps it for good. Textures uploaded inside the hook would reach the
backend a frame late (egui has already taken the frame's texture delta), so
the hook moves the new delta into `FullOutput::textures_delta` itself.

### Cost

A galley whose text has no character from U+00A9 up is skipped after one
scan of its bytes, with no allocation. The result for every other galley
is kept by galley identity (its `Arc`, held so the address cannot be
reused) from one frame to the next: a steady screen clones nothing and
segments nothing. Entries not seen in a frame are dropped. While a
cluster's picture is still on its way the galley is re-examined each frame
until it arrives.

### What stays with the app

Copying and selection work on the galley's real text, which the plugin
never changes. An app that keeps `append` (ZapFast's transcript, whose
selection maps placeholders back through `placements`) keeps working
beside the plugin. Scale factor: rectangles are in points, and each picture
is drawn at its physical size, the rectangle's height times the context's
pixels per point, as with `paint_cluster`.
