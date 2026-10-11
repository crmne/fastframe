use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

/// A platform that cannot tell holds nothing back.
#[test]
fn the_default_is_unconstrained() {
    let conditions = Conditions::default();
    assert!(conditions.online);
    assert!(!conditions.metered);
    assert!(!conditions.saver);
    assert!(!conditions.on_battery);
    assert!(!conditions.display_off);
    assert!(!conditions.locked);
}

/// Demo runs and tests get a fixed `Power`, and the device's own state
/// (Low Power Mode on a Mac) must not leak into what they see.
#[test]
fn a_fixed_monitor_keeps_its_conditions() {
    let given = Conditions {
        online: false,
        ..Conditions::default()
    };
    let mut power = Power::fixed(given);
    assert_eq!(power.conditions(), given);
    power.set_background(true);
    assert_eq!(power.conditions(), given);
    assert!(power.events().is_empty());
}

#[test]
fn a_change_wakes_the_app_once_and_a_repeat_does_not() {
    let woken = Arc::new(AtomicU32::new(0));
    let count = Arc::clone(&woken);
    let shared = Shared::new(move || {
        count.fetch_add(1, Ordering::Relaxed);
    });
    shared.update(|conditions| conditions.saver = true);
    shared.update(|conditions| conditions.saver = true);
    shared.update(|conditions| conditions.online = false);
    assert_eq!(woken.load(Ordering::Relaxed), 2);
}

/// What an app's own tests and demo runs use in place of a reader.
#[test]
fn tests_can_set_the_conditions_and_send_events() {
    let power = Power::fixed(Conditions::default());
    let metered = Conditions {
        metered: true,
        ..Conditions::default()
    };
    power.set_conditions(metered);
    assert_eq!(power.conditions(), metered);

    let ack = Ack::default();
    power.send(Event::Resumed);
    power.send(Event::Suspending(ack.clone()));
    let events = power.events();
    assert!(matches!(
        events.as_slice(),
        [Event::Resumed, Event::Suspending(_)]
    ));
    assert!(power.events().is_empty(), "each event is taken once");

    if let Some(Event::Suspending(answer)) = events.into_iter().nth(1) {
        answer.done();
    }
    assert!(
        ack.wait(Duration::ZERO),
        "the app's answer reaches the sender"
    );
}

#[test]
fn a_reader_waits_for_the_answer_but_not_forever() {
    let power = Power::fixed(Conditions::default());
    let shared = Arc::clone(&power.shared);
    let answered = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        shared.push_and_wait(Event::Suspending, Duration::from_secs(30));
        started.elapsed()
    });
    let ack = loop {
        if let Some(Event::Suspending(ack)) = power.events().pop() {
            break ack;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    ack.done();
    assert!(answered.join().unwrap() < Duration::from_secs(10));

    let started = std::time::Instant::now();
    power
        .shared
        .push_and_wait(Event::EndingSession, Duration::from_millis(30));
    assert!(started.elapsed() >= Duration::from_millis(25), "gave up");
}

/// The reader starts what must not wait for the app, and notes when the
/// system went to sleep, before it waits for the answer; a session that is
/// ending runs neither.
#[test]
fn going_to_sleep_starts_at_once_on_the_reader() {
    let power = Power::fixed(Conditions::default());
    let paused = Arc::new(AtomicU32::new(0));
    let count = Arc::clone(&paused);
    power.on_suspending(move || {
        count.fetch_add(1, Ordering::Relaxed);
    });
    let quick = Duration::from_millis(10);
    power.shared.push_and_wait(Event::EndingSession, quick);
    assert_eq!(paused.load(Ordering::Relaxed), 0);
    assert!(power.take_suspended_at().is_none());

    power.shared.push_and_wait(Event::Suspending, quick);
    assert_eq!(paused.load(Ordering::Relaxed), 1);
    assert!(power.take_suspended_at().is_some());
    assert!(power.take_suspended_at().is_none(), "taken once");
}

/// The hook runs outside the reader's lock: one that sets the next hook
/// does not hold the system back waiting on itself.
#[test]
fn a_hook_can_set_the_next_one() {
    let power = Arc::new(Power::fixed(Conditions::default()));
    let ran = Arc::new(AtomicU32::new(0));
    let (inner, count) = (Arc::clone(&power), Arc::clone(&ran));
    power.on_suspending(move || {
        count.fetch_add(1, Ordering::Relaxed);
        let count = Arc::clone(&count);
        inner.on_suspending(move || {
            count.fetch_add(10, Ordering::Relaxed);
        });
    });
    let quick = Duration::from_millis(10);
    power.shared.push_and_wait(Event::Suspending, quick);
    power.shared.push_and_wait(Event::Suspending, quick);
    assert_eq!(ran.load(Ordering::Relaxed), 11);
}

/// A hook that panics leaves the reader going: the sleep is still queued
/// for the app, and the next one runs the hook again.
#[test]
fn a_panicking_hook_still_queues_the_sleep() {
    let power = Power::fixed(Conditions::default());
    let ran = Arc::new(AtomicU32::new(0));
    let count = Arc::clone(&ran);
    power.on_suspending(move || {
        count.fetch_add(1, Ordering::Relaxed);
        panic!("the app's hook failed");
    });
    let quick = Duration::from_millis(10);
    power.shared.push_and_wait(Event::Suspending, quick);
    assert!(matches!(power.events().as_slice(), [Event::Suspending(_)]));
    power.shared.push_and_wait(Event::Suspending, quick);
    assert_eq!(ran.load(Ordering::Relaxed), 2);
}

/// An app's tests and demo runs send a sleep as a reader would, hook and
/// time included, on every platform.
#[test]
fn a_sent_sleep_runs_the_hook_and_notes_the_time() {
    let power = Power::fixed(Conditions::default());
    let ran = Arc::new(AtomicU32::new(0));
    let count = Arc::clone(&ran);
    power.on_suspending(move || {
        count.fetch_add(1, Ordering::Relaxed);
    });
    power.send(Event::Resumed);
    assert_eq!(ran.load(Ordering::Relaxed), 0);
    assert!(power.take_suspended_at().is_none());

    power.send(Event::Suspending(Ack::default()));
    assert_eq!(ran.load(Ordering::Relaxed), 1);
    assert!(power.take_suspended_at().is_some());
    assert!(matches!(
        power.events().as_slice(),
        [Event::Resumed, Event::Suspending(_)]
    ));
}

/// An app's tests check that it answered what it was sent.
#[test]
fn an_answer_can_be_checked() {
    let ack = Ack::default();
    assert!(!ack.is_done());
    ack.clone().done();
    assert!(ack.is_done());
}

#[test]
fn events_read_plainly_in_the_log() {
    let event = Event::Suspending(Ack::default());
    assert_eq!(format!("{event:?}"), "Suspending(Ack)");
    assert_eq!(format!("{:?}", Activity::begin("test")), "Activity");
}

/// An app keeps both in state that may move between threads (a worker
/// that owns the player, say), on every platform alike.
#[test]
fn power_and_activities_can_move_between_threads() {
    fn send_and_sync<T: Send + Sync>() {}
    send_and_sync::<Power>();
    send_and_sync::<Activity>();
    send_and_sync::<Ack>();
}

/// Beginning and ending activities, one inside another, as playback starts
/// and stops. On macOS this asks NSProcessInfo; elsewhere it does nothing.
#[test]
fn activities_begin_and_end() {
    let outer = Activity::begin("Playing audio");
    let inner = Activity::begin("Loading a track");
    drop(inner);
    drop(outer);
}
