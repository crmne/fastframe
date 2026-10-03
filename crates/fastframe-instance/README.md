# fastframe-instance

One running copy of an app per user. A second launch, from the launcher, a
link the desktop opens, or a command line, hands its request to the running
copy and exits.

The running copy holds an exclusive lock on a file in a private per-user
directory, its slot. The operating system releases the lock when the process
ends, even after a crash, so nothing left behind blocks a later launch, and
two launches at the same moment still make only one running copy. Requests
travel over a private channel:

- **Unix:** a socket in the slot's directory, which only the user can open
  (the directory is `0700`, the socket `0600`).
- **Windows:** a loopback port, which any local process can reach, so every
  request must first present a random token the running copy writes to the
  slot's directory in the user's profile. A browser that reaches localhost
  never has the token.

Requests and replies are single lines that start with the app's name, so a
copy of one app never obeys another's. A request is whatever the app
understands (`show`, `open-link spotify:album:...`, `next`), up to 16 KiB,
and the reply is whatever the app answers, such as a now-playing snapshot.
The crate does not parse requests: each app keeps its own verbs.

It came out of ZapFast's single-instance guard, with the requests and
replies Spotifast's remote control needs.

## Usage

```rust
use fastframe_instance::{Claim, Slot};

let slot = Slot::new("rocks.example.App");
let request = if start_hidden { "ping" } else { "show" };
let guard = match slot.claim(request, move |request| match request {
    "show" => {
        queue.push(Command::Show);
        waker.wake();
        Some("ok".to_owned())
    }
    "ping" => Some("ok".to_owned()),
    _ => None, // refused: the launch gets no reply
}) {
    Claim::First(guard) => guard,       // this is the running copy
    Claim::Running(_reply) => return,   // the running copy took the request
    Claim::Unanswered => return,        // running, but no answer in ANSWER_WAIT
};
// Keep `guard` for the life of the process.
```

The handler runs on a thread of its own, one request at a time: queue the
request for the app and wake it, rather than doing the work there.

`Slot::send` only sends, for a command-line action that makes sense only
while the app runs (`app reload-themes`). An error of kind `NotFound` or
`ConnectionRefused` means no copy is running.

## Slots

- `Slot::new(app_id)` is the one slot per user: `$XDG_RUNTIME_DIR/<app_id>`
  on Linux (the app's own runtime directory inside Flatpak), the user's
  private `$TMPDIR` on macOS (an Application Support path can outgrow the
  104 bytes a socket path has there), and the user's local data directory on
  Windows.
- `.scoped("demo")` is a separate slot beside it, for a copy that should run
  alongside the real app, such as a demo or a test.
- `Slot::at(dir, name)` puts the slot in a directory the app already uses,
  with `name` as the line prefix. An app moving onto this crate can keep its
  directory and its prefix, so a new launch still reaches an older copy
  that is already running.

A slot whose lock cannot be taken at all (a read-only directory, say) runs
unguarded rather than not at all, with a warning in the log.
