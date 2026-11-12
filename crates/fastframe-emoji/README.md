# fastframe-emoji

Colour emoji in the platform's own style, inline in egui text: Apple's on
macOS, Segoe UI Emoji on Windows, the desktop's emoji font on Linux, and a
font the app bundles for anything they lack.

- **Whole sequences**: flags, skin tones, keycaps and families joined with
  U+200D become one picture, as the platform joins them. A sequence newer
  than the fonts shows its first emoji.
- **Inline**: each emoji is laid out as one transparent placeholder glyph,
  as wide as the picture painted over it and no taller than the text row,
  so selection, wrapping and right-to-left reordering work on it like any
  character.
- **Never in the frame**: fonts are memory-mapped once (Apple's is 190 MB),
  and each new picture is drawn on a worker thread; the frame that first
  shows an emoji leaves its placeholder and the worker asks for a repaint.
- **Outside egui too**: `Emoji::render` hands back a premultiplied RGBA
  picture for an app's own renderer.

## Usage

```rust
// At startup: the platform's emoji, with the app's own font behind them.
fastframe_emoji::EmojiSetup::default()
    .bundled(include_bytes!("../assets/fonts/NotoColorEmoji.ttf"))
    .install();
std::thread::spawn(fastframe_emoji::warm_up);
```

### Every text, with no layout code

```rust
ctx.add_plugin(fastframe_emoji::EmojiPlugin::default());
```

The plugin looks at the frame's text after every widget has painted:
labels, buttons, menus, tooltips, text fields, text an app paints itself.
Each emoji's monochrome glyph is hidden and its picture painted in the
glyph's place, a little taller than the row and clipped like the text, so
layout, wrapping, elision and selection stay egui's own. A picture still
on its way leaves the monochrome glyph for that frame. Text drawn outside
egui's text shapes (an app's own renderer) is out of its reach; see
`Emoji::render`.

The picture fills the space egui laid the emoji out in. The apps' egui
fork (crmne/egui `apps-0.36`) lays every emoji cluster out one emoji wide,
however the monochrome font draws it, so a family, a keycap and a 😀 are the
same size and cursors, selection and widget sizes agree with the pictures.
Stock egui 0.36 lays a sequence the monochrome font does not join out as
its parts (a family as three people, a keycap as its digit), and the
picture fills that wider or narrower space. Lay out text where that matters
with `append`, which gives every emoji one placeholder's width.

### Selectable placeholders

For text whose copied selection must turn each emoji back into its
characters (ZapFast's transcript), lay the emoji out as placeholders. The
plugin leaves these to the app.

```rust
// Laying out a message:
let mut job = egui::text::LayoutJob::default();
let mut placements = Vec::new();
fastframe_emoji::append(ui, &mut job, &mut placements, text, &format);
let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
// ...paint the galley, then the pictures over it:
fastframe_emoji::paint(ui, &galley, origin, &placements);
```

A text editor keeps its characters: `editor_job` draws the emoji
transparent and returns where they are for `paint_cluster`. A copied
selection holds `PLACEHOLDER` for each emoji; `placements` lists the
clusters in the same order, to put them back.

Apps choose:

```rust
EmojiSetup::default()
    .system(true)        // the platform's emoji font (the default)
    .bundled(noto)       // a CBDT or sbix font for what it lacks
    .synchronous(demo)   // draw inside the frame, for screenshot tests
    .install();
fastframe_emoji::prewarm(ctx, picker_page);   // queue pictures early
let picture = fastframe_emoji::get().render("👍🏽", 48); // outside egui
```

## Where pictures come from

| Platform | System font | Joined by | Drawn by |
| --- | --- | --- | --- |
| Linux | fontconfig's `emoji:color=true` (asked through `fc-match`), else a colour bitmap face found in the font directories, in fontconfig's order of preference | harfrust (GSUB) | skrifa's CBDT strikes |
| macOS | `/System/Library/Fonts/Apple Color Emoji.ttc`, mapped | harfrust (AAT `morx`) | skrifa's sbix strikes |
| Windows | Segoe UI Emoji | DirectWrite | Direct2D (COLR) |

The bundled font is shaped and drawn like the Linux one. Each cluster is
tried whole, then without U+FE0F, then with trailing parts dropped, and
every font is asked for each form before a part is dropped. Segoe UI Emoji
has no country or subdivision flags, so on Windows those come from the
bundled font.

Pictures are framed in the emoji font's own cell (one advance wide, ascent
plus descent tall), so each platform's emoji sit on the text as that
platform draws them. See [DESIGN.md](DESIGN.md) for the reasoning.

## What stays in the app

The bundled font and its licence, the text sizes that set the size emoji are
drawn at, and how the app turns placeholders back into text when copying
(ZapFast's transcript).

## Credits

The layout with placeholders and the bitmap reading come from ZapFast. The
Windows renderer is Andrés Rodríguez's (@pulgueta, ZapFast #229), and
joining Apple's sequences with harfrust over a mapped font is GM's
(@thisisgm, ZapFast #279).

## Licences

The code is MIT. The test fixture is a subset of Noto Color Emoji, under
the SIL Open Font License 1.1.
