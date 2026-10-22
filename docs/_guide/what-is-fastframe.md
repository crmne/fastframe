---
title: What is fastframe?
description: Why fastframe exists, which apps it serves, and the rules for what goes in it.
nav_order: 1
---

fastframe is the shared foundation of a family of native desktop apps built
with Rust and [egui](https://github.com/emilk/egui):

- [ZapFast](https://zapfast.rocks), a WhatsApp client
- [Spotifast](https://spotifast.rocks), a Spotify client
- [Solco](https://getsolco.com), a DJ library manager for Pioneer and
  AlphaTheta hardware
- [TonePush](https://docs.tonepush.rocks), an editor for Line 6 HX pedals and
  the StompStation PRO

Each of these apps is a single native binary with no browser engine, running on
Linux, macOS and Windows. Underneath their interfaces they need the same
things: text that renders like the rest of the desktop, fonts for every script,
icons, colour themes, translations, a log file for bug reports, a tray item, a
way to keep running with the window closed, and a way to update themselves.

[Chat with Work Desktop](https://github.com/crmne/chatwithwork-desktop),
the desktop app for [Chat with Work](https://chatwithwork.com), was one of
them too, and fastframe was partly extracted from its egui app. It is now a
Tauri app whose window is the Chat with Work web app, and it still uses
fastframe's audio, notifications and start-at-login crates.

Every piece here was written for one of those apps first and moved here once
another app needed it. It is a Cargo workspace of small `fastframe-*` crates,
so an app takes only what it uses.

## Principles

- **Extract from working apps, never design up front.** Code arrives here
  after it has shipped in an app, not before.
- **Two apps, then it moves.** A piece moves in only once at least two apps
  have it.
- **Small crates, one workspace.** Each piece is its own `fastframe-*` crate,
  so an app takes only what it uses.
- **Good defaults, few knobs.** A crate does the right thing with no
  configuration. Apps keep their own decisions (branding, layout, product
  choices); fastframe does not make them.
- **No telemetry, no hosted services.** Nothing here phones home or depends
  on a server run by us.
- **Fix upstream.** Bugs in egui, winit and friends are fixed in those
  projects, not carried here as patches forever.

## What stays in the apps

fastframe owns what is the same in every app. Each app keeps its interface,
its layout, its branding, its palette type and how it maps onto egui's
visuals, its wording, and when it checks for updates. Each crate page ends
with what it deliberately leaves to the app, and why.

## Status

Early, extracted piece by piece. APIs will change while the first crates
settle. fastframe publishes [GitHub releases](https://github.com/crmne/fastframe/releases),
and apps depend on a release tag; nothing is on crates.io yet.
