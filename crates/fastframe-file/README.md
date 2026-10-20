# fastframe-file

Replaces a file on disk without ever leaving it half written.

Settings, sessions and credentials are rewritten whole. Writing over the file
in place leaves it truncated if the app is killed or the power goes halfway
through, and the app then starts from defaults. `write` writes a temporary
file beside the target instead, flushes it to the disk, and moves it over the
target in one step, so a reader sees the old contents or the new ones, never
part of either.

Flushing to the disk can take tens of milliseconds, and seconds on a busy or
network drive. The thread that draws the window must never wait for it, or
the window stops drawing and every control waits with it. A `Writer` takes
the bytes and writes them on a thread of its own.

## Usage

```rust
fastframe_file::write(&dirs.settings_file(), serde_json::to_vec_pretty(&settings)?)?;

// A secret only the user may read (0600 on Unix):
fastframe_file::write_private(&dirs.token_file(), token.as_bytes())?;

// Or stream into the file, from a serializer say:
fastframe_file::write_with(&path, fastframe_file::Access::Default, |file| {
    serde_json::to_writer(file, &session).map_err(std::io::Error::other)
})?;
```

## Saving from the UI thread

```rust
// Once, on the thread that draws the window (in eframe's app creator):
fastframe_file::mark_ui_thread();

let files = fastframe_file::Writer::new();
// Encode on the UI thread (cheap), and hand the bytes over. Returns at once.
files.write(&dirs.settings_file(), serde_json::to_vec_pretty(&settings)?, Access::Default);
// Hear about a failure, to write again on the next save:
files.write_or(&dirs.history_file(), text, Access::Private, move || retry.store(true, Relaxed));

// Before the app exits:
files.flush(fastframe_file::FLUSH_TIMEOUT);
// Before acknowledging a suspend or the end of the session, within the time
// the system waits for the app:
files.flush(Duration::from_millis(1500));
ack.done();
```

- Writes run in order on one thread, named `file-writer`, with the same
  guarantees as `write` and `write_private`.
- A newer write to a file whose previous write has not started yet replaces
  it, so only the newest contents reach the disk. Writes of other files keep
  their places.
- `flush` waits until everything handed over is written, or the timeout
  passes, and says which. A timeout is logged.
- A failed write is logged, and `write_or` calls its closure on the writer's
  thread. A write replaced by a newer one never calls it.
- Dropping the writer lets its thread finish what it holds and stop. Flush
  first if the writes must be done.
- `Writer::with_write` takes the function that writes, for tests that hold a
  write back or watch what is written. Tests that read a saved file back
  flush first.

After `mark_ui_thread`, `write`, `write_private` and `write_with` refuse that
thread: a debug build (and so a test) panics, and a release build logs an
error and still writes, rather than lose what was saved. The `Writer` itself
never trips it.

## What it does

1. Creates the file's directory if it is missing (with mode 0700 for a
   private file on Unix, so others cannot list it there either).
2. Writes into `.<name>.<pid>.<n>.tmp` beside the file, a name no other write
   uses at the same time, created fresh (with mode 0600 for a private file).
3. Flushes it to the disk, so a crash right after the move cannot leave the
   name pointing at nothing.
4. Moves it over the file: `rename` on Unix, and on Windows `MoveFileExW`
   with write-through, so the move itself is on the disk when it returns.
5. On Unix, syncs the directory, where the rename lives.

If writing or any later step fails, the file keeps its old contents and the
temporary file is removed.

It came from Spotifast's saves (the Windows write-through move), Solco's
licence file (the flush, the directory sync and the private mode), and
ZapFast's settings, which wrote and renamed without flushing. The `Writer`
came from Spotifast, whose window froze for seconds while btrfs committed its
log under the settings, session and history saves.
