---
layout: home
title: fastframe
description: egui on rails. The shared foundation of a family of native Rust desktop apps for Linux, macOS and Windows.
permalink: /
hero:
  name: fastframe
  text: egui on rails
  tagline: >-
    The small crates behind <a href="https://zapfast.rocks">ZapFast</a>,
    <a href="https://spotifast.rocks">Spotifast</a>, <a href="https://getsolco.com">Solco</a>,
    <a href="https://docs.tonepush.rocks">TonePush</a> and <a href="https://github.com/crmne/chatwithwork-desktop">Chat with Work Desktop</a>.
    Everything a native egui app needs besides its own interface, extracted from apps that already ship it.
  actions:
    - theme: brand
      text: Get started
      link: /getting-started/
    - theme: alt
      text: What is fastframe?
      link: /what-is-fastframe/
    - theme: alt
      text: GitHub
      link: https://github.com/crmne/fastframe
  image:
    src: /assets/images/logo.svg
    alt: fastframe logo
    width: 320
    height: 320

features:
  - icon: 🔤
    title: Text that looks native
    details: Follows the desktop's hinting and antialiasing, bundles Inter with tabular figures, and finds installed fonts for every script Inter lacks.
    link: /fastframe-text/
    link_text: fastframe-text
  - icon: 🎨
    title: Themes people can edit
    details: JSON palettes loaded off the interface thread, eight shared palettes, light and dark pairs that follow the system, and live Omarchy theme following with a reveal from the middle of the window.
    link: /fastframe-theme/
    link_text: fastframe-theme
  - icon: 🪟
    title: Runs without a window
    details: A tray item on every platform, a headless loop around eframe that brings the window back on demand, and windows rescued from off-screen.
    link: /fastframe-shell/
    link_text: fastframe-shell
  - icon: 🔄
    title: Updates that roll back
    details: Self-update from GitHub releases or signed HTTPS feeds, package-manager detection, and a helper that restores the previous version if the new one does not start.
    link: /fastframe-update/
    link_text: fastframe-update
  - icon: 🌍
    title: Translations built in
    details: gettext catalogs compiled into the binary at build time. No libintl, no PO parsing at run time, English as the fallback.
    link: /fastframe-i18n/
    link_text: fastframe-i18n
  - icon: 🔒
    title: Private by default
    details: Logs made for bug reports, with private targets rewritten and panic payloads never written. No telemetry, no hosted services, nothing that phones home.
    link: /fastframe-log/
    link_text: fastframe-log
---

<style>
  /* The app names in the hero tagline are links; the theme draws tagline
     text without link styling. */
  .VPHero .tagline a {
    color: inherit;
    text-decoration: underline;
    text-decoration-color: var(--vp-c-brand-1);
    text-decoration-thickness: 2px;
    text-underline-offset: 4px;
    transition: color 0.25s;
  }
  .VPHero .tagline a:hover {
    color: var(--vp-c-brand-1);
  }
</style>

<div class="vp-doc" style="max-width: 1152px; margin: 4rem auto 0; padding: 0 24px;">
  <h2 style="border-top: none; margin-top: 0; padding-top: 0;">The crates</h2>
  <p>Take only the crates your app uses. Each one does one job and does it with no configuration.</p>
  <table>
    <thead><tr><th>Crate</th><th>What it does</th></tr></thead>
    <tbody>
      {% assign crates = site.crates | sort: 'nav_order' %}
      {% for crate in crates %}
      <tr><td><a href="{{ crate.url | relative_url }}"><code>{{ crate.title }}</code></a></td><td>{{ crate.description }}</td></tr>
      {% endfor %}
    </tbody>
  </table>
</div>

<div class="vp-doc" style="max-width: 1152px; margin: 3rem auto; padding: 0 24px;">
  <h2>Native text, in both themes</h2>
  <p>The font specimen is a real egui window with synthetic text. Try the default
  interface face or load a validated local font, with the same weight roles and script fallbacks.</p>
  <p><img src="{{ '/assets/images/fonts-dark.png' | relative_url }}" alt="Dark native font specimen with multilingual script fallbacks" loading="lazy" width="1000" height="620"></p>
  <p><img src="{{ '/assets/images/fonts-light.png' | relative_url }}" alt="Light native font specimen with a local font" loading="lazy" width="1000" height="620"></p>
</div>
