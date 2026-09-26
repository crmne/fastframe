# fastframe-theme

Colour palettes for egui apps: JSON palette files, a catalogue loaded off
the interface thread, the palettes the apps share, and following the
[Omarchy](https://omarchy.org) desktop's theme live.

The app keeps its own palette type (its colours and its dark and light
defaults) and its mapping onto `egui::Visuals` and its widgets. This crate
owns what is the same in every app: reading palette files, listing them,
the shared palettes, and Omarchy.

## Palettes

```rust
impl fastframe_theme::Palette for Palette {
    fn base(base: fastframe_theme::Base) -> Self {
        match base {
            fastframe_theme::Base::Dark => Palette::dark(),
            fastframe_theme::Base::Light => Palette::light(),
        }
    }

    fn set(&mut self, name: &str, color: Color32) -> bool {
        match name {
            "window" => self.window = color,
            "accent" => self.accent = color,
            // ... every colour the app has ...
            _ => return false,
        }
        true
    }

    // Optional: fill app-only colours from the base ones a file set.
    fn derive(&mut self, given: &BTreeSet<&str>) {
        if given.contains("window") && !given.contains("chat") {
            self.chat = self.window;
        }
    }
}
```

A palette file names a `base` (`dark`, the default, or `light`) and the
colours it overrides, as `#RRGGBB` or `#RRGGBBAA`:

```json
{ "base": "light", "colors": { "accent": "#907aa9", "danger": "#b4637a" } }
```

`BASE_COLORS` lists the sixteen names every app reads (`window`, `panel`,
`surface`, `surface_hover`, `surface_active`, `outline`, `text`,
`secondary`, `dim`, `accent`, `accent_hover`, `on_accent`, `danger`,
`warning`, `overlay`, `shadow`); an app may accept more. Unknown names and
fields make a file invalid, so a typo is reported rather than ignored.

`parse_palette::<P>(text)` reads one; `CustomTheme<P>` pairs it with its
filename and serializes as `{ "filename": .., "palette": .. }`;
`read_cached_theme` reads a cached one from settings, treating a damaged
cache as absent.

## The catalogue

```rust
// App state:
custom_themes: fastframe_theme::Catalog<Palette>,

// Normal launches (not demos):
app.custom_themes.enable_desktop_themes(fastframe_theme::DesktopThemes {
    slug: "zapfast",
    omarchy_template: include_str!("../contrib/omarchy/zapfast.json.tpl"),
    presets: true,
});

// At start, on "reload-themes", and when needs_reload() says so:
let waker = fastframe_theme::Waker::new(move || app_waker.wake());
app.custom_themes.start(config_dir.join("themes"), settings.custom_theme.clone(), &waker);

// Each frame (or on wake-up):
if app.custom_themes.needs_reload() { app.load_custom_themes(); }
if app.custom_themes.poll() { /* re-resolve the selected palette */ }
```

- The themes directory is listed on a background thread: at most 512
  entries, 128 palettes, 64 KiB per file, regular files only (no links, no
  paths out of the directory). The selected file is read first, so a large
  directory cannot push it out.
- One scan runs and one request waits; a burst of reloads collapses into
  the last, and a superseded result is never published, so colours do not
  flash back.
- `picker_themes()` lists the live Omarchy palette first when it is followed;
  `status(selected)` says what to show under the setting (`Loading`,
  `SelectedUnavailable`, or a `Problem`), which the app words and translates.
- `Catalog::preview(themes, follows_omarchy)` builds one for demos and tests
  without touching the desktop.

## Shared palettes

`presets` embeds eight palettes in the sixteen base colours: Catppuccin,
Catppuccin Latte, Nord, Ristretto, Rosé Pine, Rosé Pine Moon, Rosé Pine Dawn
and Tokyo Night. With `presets: true` they are listed alongside the user's
files; a user file with the same name overrides one. They create no files.

## Omarchy

On Linux, when `~/.config/omarchy` and `~/.local/state/omarchy/current`
exist, the catalogue follows the desktop:

1. It reads the palette Omarchy rendered for the app
   (`current/theme/<slug>.json`); without one it renders the user's template
   (`~/.config/omarchy/themed/<slug>.json.tpl`) or the app's own, from
   `omarchy-theme-color --all`. Source and portable builds follow too.
2. It lists that palette as `omarchy.json` ("Omarchy" in the picker) and
   offers it as `system_theme()`.
3. It watches the themes directory and Omarchy's current theme with
   filesystem notifications; `needs_reload()` turns true on a change, with
   no repaint timer.

A native package may also ship `share/<slug>/omarchy/<slug>.json.tpl` and
the hook `share/<slug>/omarchy/<slug>-theme`. The first scan then copies
them to `~/.config/omarchy/themed/` and `~/.config/omarchy/hooks/theme-set.d/`
and seeds `themes/omarchy.json`, never replacing an existing file (or even a
broken link). Omarchy then renders the palette itself on every theme
change, and the hook asks a running app to reload through
`<slug> reload-themes`, which the app answers over its single-instance
channel.

### Packaging files the apps ship

These stay in each app and are installed by its packages:

| File | Installed to | What it is |
| --- | --- | --- |
| `contrib/omarchy/<slug>.json.tpl` | `share/<slug>/omarchy/` | The Omarchy template. `BASE_TEMPLATE` is the sixteen-colour one (Spotifast's); ZapFast's adds `chat`, `bubble_in`, `bubble_out`, `link` and `read`. |
| `contrib/omarchy/<slug>-theme` | `share/<slug>/omarchy/` | The `theme-set` hook. `omarchy::hook_script(slug)` returns its exact text; test the shipped file against it so they cannot drift. |

The hook honours `<SLUG>_OMARCHY_THEME_DIR` and `<SLUG>_THEMES_DIR` so it
can be tested without touching the desktop.

On macOS and Windows the catalogue lists files and presets only;
`follows_omarchy()` is false and `needs_reload()` never fires.

## Revealing a change of colours

`Transition` reveals new colours from the middle of the window outwards, as
Omarchy does when its theme changes. Keep one per window. When the palette
is about to change, call `begin`, and keep drawing the old palette while
`holding` says so; when it stops holding, apply the new one. Call `paint`
last in every frame.

```rust
if app.applied != app.wanted {
    app.transition.begin(ctx);
    if !app.transition.holding(ctx) {
        app.apply(app.wanted);
    }
}
// ... draw the interface, then:
app.transition.paint(ctx);
```

`begin` asks the window for a screenshot with the old colours. When it
arrives, `paint` lays it over the new colours with an opening cut out of its
middle that widens until the old colours are gone. The default,
`Reveal::Band`, is Omarchy's own theme change, taken from its background
shell: a band leaning slightly to the right (a slant of -0.18 of the
window's height) opens from the middle towards both sides over 0.42
seconds, easing in and out, with a sharp anti-aliased edge.
`Transition::new(Reveal::Circle)` grows a soft-edged circle from the middle
past the corners over 0.6 seconds instead. A window that sends no
screenshot (hidden, or a renderer without them) gets its new colours after
a quarter of a second, without the reveal. Skip `begin` for the first
palette a window draws.
