# fastframe-autostart

Start an app when the person logs in, with its window or hidden in the tray.

```rust
use fastframe_autostart::{Autostart, Start, State};

let autostart = Autostart::new("spotifast", "Spotifast");

// Both block (files, the registry, SMAppService, the portal): call them on a
// worker, never the interface thread.
let state = autostart.state(); // Unsupported, Off, On(start) or NeedsApproval(start)
let state = autostart.set(Some(Start::Hidden))?; // or Some(Start::Window), or None
```

The platform's own record is the only one: `state` reads it back, so a change
the person makes in System Settings, Task Manager or the desktop shows up in
the app, and the app keeps no copy that could disagree.

| Platform | Start with the window | Start hidden |
| --- | --- | --- |
| Linux | `$XDG_CONFIG_HOME/autostart/<id>.desktop`, written atomically | the same entry, with `--start-hidden` on its `Exec` line |
| Flatpak | the background portal's `RequestBackground` with `autostart` and `commandline` | the same, with `--start-hidden` |
| macOS 13+ | `SMAppService.mainApp`, "Open at Login" | `SMAppService.agent` with a launch agent the bundle carries |
| Windows | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, the quoted executable | the same value, with `--start-hidden` |

- **Linux** reads the entry back, and treats `Hidden=true` or
  `X-GNOME-Autostart-enabled=false` (how desktops turn an entry off without
  deleting it) as off. An AppImage registers its `$APPIMAGE` path, not the
  temporary mount it runs from.
- **Flatpak** cannot write the host's autostart folder, so it asks the portal,
  which may ask the person first, and is told no with
  `io::ErrorKind::PermissionDenied`. The portal cannot be asked what it
  registered, so the crate keeps the same entry in the sandbox's own
  configuration folder (`~/.var/app/<app id>/config/autostart`) as the record;
  a change made in the desktop's own settings is not seen there.
- **macOS** cannot pass arguments to a login item, so a hidden start is a
  launch agent in the bundle at `Contents/Library/LaunchAgents/`
  `macos_agent_plist_name(bundle id)`, whose text `macos_agent_plist(bundle id,
  "--start-hidden")` gives. The packaging script writes it before signing. The
  agent runs `open -g -b <bundle id> --args --start-hidden` and exits:
  unregistering an agent kills its running process, which must never be the
  app itself. `NeedsApproval` means the person has to allow it in System
  Settings, General, Login Items. An unbundled run (`cargo run`) and macOS 12
  or earlier are `Unsupported`; a bundle without the agent can still start
  with its window.
- **Windows** writes the value named after the app. An entry turned off in Task
  Manager's Startup apps (its `StartupApproved` value) reads as off, and is
  cleared when the app registers again.

A hidden start passes `START_HIDDEN` (`--start-hidden`), or the app's own
argument through `Autostart::hidden_argument`. Hand it to
`fastframe_shell::Shell::start_hidden`; the app's `Resident::start_hidden`
opens the window after all when there is no tray to come back from.

It came from ZapFast (start in the tray at login) and Chat with Work Desktop
(start in the background at login), which each wrote an XDG entry, a
LaunchAgent in `~/Library/LaunchAgents` and the Run value, and from
Spotifast's request to choose between starting with the window and in the
tray (crmne/spotifast#122).
