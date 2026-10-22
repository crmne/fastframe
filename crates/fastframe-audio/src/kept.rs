//! A connection the whole process shares, made when it is first needed and
//! made again only after it fails.
//!
//! PipeWire's default devices are looked up through its PulseAudio server
//! (`stream::server_default`). A new connection for every lookup cost a
//! thread and a socket each, and pulseaudio-rs logs an error for every
//! connection it closes, so an app that relists its devices every few
//! seconds filled its log (ZapFast: about 23 lines a minute).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// What a question returns when the connection it asked has failed: the
/// connection is forgotten, and the next question makes a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Broken;

/// The kept connection, if one has been made and has not failed.
#[derive(Debug)]
pub(crate) struct Kept<T>(Mutex<Option<Arc<T>>>);

impl<T> Kept<T> {
    /// No connection yet.
    pub(crate) const fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// Asks the kept connection, made with `connect` when there is none. A
    /// connection that turns out broken is forgotten and the question asked
    /// once more of a new one, so a restarted server is reached on the same
    /// call. `None` when no connection can be made, or the new one is broken
    /// too; the next call tries again.
    ///
    /// The lock is held only to take the connection or to forget it, never
    /// while connecting or asking, which can each take a while (up to two
    /// seconds for a PulseAudio server that does not answer). A slow server
    /// holds up only the threads asking it, at most as long as each would
    /// have waited for a connection of its own, and the audio callbacks never
    /// ask at all.
    pub(crate) fn ask<R>(
        &self,
        connect: impl Fn() -> Option<T>,
        ask: impl Fn(&T) -> Result<R, Broken>,
    ) -> Option<R> {
        for _ in 0..2 {
            let connection = self.get(&connect)?;
            match ask(&connection) {
                Ok(answer) => return Some(answer),
                Err(Broken) => self.forget(&connection),
            }
        }
        None
    }

    /// The kept connection, or a new one, kept unless another thread kept
    /// one while this one connected (then this one is dropped).
    fn get(&self, connect: impl Fn() -> Option<T>) -> Option<Arc<T>> {
        if let Some(kept) = self.lock().as_ref() {
            return Some(Arc::clone(kept));
        }
        let made = Arc::new(connect()?);
        Some(Arc::clone(self.lock().get_or_insert(made)))
    }

    /// Forgets `broken`, unless another thread has already replaced it.
    fn forget(&self, broken: &Arc<T>) {
        let mut kept = self.lock();
        if kept.as_ref().is_some_and(|kept| Arc::ptr_eq(kept, broken)) {
            *kept = None;
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Arc<T>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    /// A stand-in for a server connection that can be made to fail.
    #[derive(Debug)]
    struct Fake {
        number: usize,
        alive: AtomicBool,
    }

    /// Makes numbered connections and counts them.
    #[derive(Debug, Default)]
    struct Server {
        made: AtomicUsize,
        down: AtomicBool,
    }

    impl Server {
        fn connect(&self) -> Option<Fake> {
            if self.down.load(Ordering::SeqCst) {
                return None;
            }
            Some(Fake {
                number: self.made.fetch_add(1, Ordering::SeqCst) + 1,
                alive: AtomicBool::new(true),
            })
        }

        fn made(&self) -> usize {
            self.made.load(Ordering::SeqCst)
        }
    }

    fn answer(fake: &Fake) -> Result<usize, Broken> {
        if fake.alive.load(Ordering::SeqCst) {
            Ok(fake.number)
        } else {
            Err(Broken)
        }
    }

    #[test]
    fn many_questions_share_one_connection() {
        let server = Server::default();
        let kept = Kept::new();
        for _ in 0..100 {
            assert_eq!(kept.ask(|| server.connect(), answer), Some(1));
        }
        assert_eq!(server.made(), 1);
    }

    #[test]
    fn a_broken_connection_is_replaced_within_the_same_question() {
        let server = Server::default();
        let kept = Kept::new();
        assert_eq!(kept.ask(|| server.connect(), answer), Some(1));
        // The server restarted: the kept connection fails from now on.
        let first = kept.get(|| server.connect()).expect("kept");
        first.alive.store(false, Ordering::SeqCst);
        assert_eq!(kept.ask(|| server.connect(), answer), Some(2));
        assert_eq!(kept.ask(|| server.connect(), answer), Some(2));
        assert_eq!(server.made(), 2);
    }

    #[test]
    fn a_server_that_is_down_is_tried_again_on_the_next_question() {
        let server = Server::default();
        let kept = Kept::new();
        server.down.store(true, Ordering::SeqCst);
        assert_eq!(kept.ask(|| server.connect(), answer), None);
        assert_eq!(kept.ask(|| server.connect(), answer), None);
        server.down.store(false, Ordering::SeqCst);
        assert_eq!(kept.ask(|| server.connect(), answer), Some(1));
    }

    #[test]
    fn a_question_that_breaks_every_connection_gives_up_after_two() {
        let server = Server::default();
        let kept = Kept::new();
        assert_eq!(
            kept.ask(|| server.connect(), |_| Err::<(), _>(Broken)),
            None
        );
        assert_eq!(server.made(), 2);
        // Nothing broken is kept: the next question connects afresh.
        assert_eq!(kept.ask(|| server.connect(), answer), Some(3));
    }

    #[test]
    fn a_stale_failure_does_not_forget_the_connection_that_replaced_it() {
        let server = Server::default();
        let kept = Kept::new();
        let first = kept.get(|| server.connect()).expect("connects");
        kept.forget(&first);
        let second = kept.get(|| server.connect()).expect("connects");
        // A thread still holding the first one finds it broken late.
        kept.forget(&first);
        let still = kept.get(|| server.connect()).expect("kept");
        assert!(Arc::ptr_eq(&still, &second));
        assert_eq!(server.made(), 2);
    }

    #[test]
    fn asking_does_not_hold_the_lock_while_the_server_is_slow() {
        let server = Arc::new(Server::default());
        let kept = Arc::new(Kept::new());
        assert_eq!(kept.ask(|| server.connect(), answer), Some(1));
        let (asking, asked) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let slow = {
            let (server, kept) = (Arc::clone(&server), Arc::clone(&kept));
            std::thread::spawn(move || {
                kept.ask(
                    || server.connect(),
                    |fake| {
                        let _ = asking.send(());
                        // A server that takes its time to answer, until
                        // released (or the test fails and drops `release`).
                        let _ = released.recv();
                        answer(fake)
                    },
                )
            })
        };
        asked
            .recv_timeout(Duration::from_secs(10))
            .expect("the slow question started");
        // Another thread is answered while the first still waits. Were the
        // lock held across the question, this one would wait until release.
        let (answered, answer_came) = mpsc::channel();
        {
            let (server, kept) = (Arc::clone(&server), Arc::clone(&kept));
            std::thread::spawn(move || {
                let _ = answered.send(kept.ask(|| server.connect(), answer));
            });
        }
        assert_eq!(
            answer_came.recv_timeout(Duration::from_secs(5)),
            Ok(Some(1))
        );
        let _ = release.send(());
        assert_eq!(slow.join().expect("no panic"), Some(1));
        assert_eq!(server.made(), 1);
    }

    #[test]
    fn threads_asking_at_once_all_get_answers_and_end_up_sharing_one() {
        let server = Arc::new(Server::default());
        let kept = Arc::new(Kept::new());
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (server, kept) = (Arc::clone(&server), Arc::clone(&kept));
                std::thread::spawn(move || {
                    (0..200).all(|_| kept.ask(|| server.connect(), answer).is_some())
                })
            })
            .collect();
        for thread in threads {
            assert!(thread.join().expect("no panic"));
        }
        // Threads that raced to connect first may each have made one, but
        // only one is kept and every later question uses it.
        assert!(server.made() <= 8);
        let kept_one = kept.get(|| server.connect()).expect("kept").number;
        assert_eq!(kept.ask(|| server.connect(), answer), Some(kept_one));
    }
}
