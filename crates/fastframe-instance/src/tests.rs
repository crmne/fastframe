use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::{Arc, Barrier, Mutex};

use super::*;

/// A slot in a throwaway directory, which goes away with the returned guard.
fn slot() -> (tempfile::TempDir, Slot) {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let slot = Slot::at(dir.path().join("run"), "test.app");
    (dir, slot)
}

/// A request handler that records what it got and accepts `show` and
/// `ping`, answering `now` with a snapshot.
type Seen = Arc<Mutex<Vec<String>>>;

fn recorder() -> (Seen, impl FnMut(&str) -> Option<String> + Send + 'static) {
    let seen: Seen = Arc::default();
    let log = Arc::clone(&seen);
    let handle = move |request: &str| {
        log.lock().unwrap().push(request.to_owned());
        match request {
            "show" | "ping" => Some("ok".to_owned()),
            "now" => Some("playing\tGo".to_owned()),
            _ => None,
        }
    };
    (seen, handle)
}

#[test]
fn a_second_launch_hands_its_request_to_the_first() {
    let (_dir, slot) = slot();
    let (seen, handle) = recorder();
    let Claim::First(_guard) = slot.claim("show", handle) else {
        panic!("the first launch is the running copy");
    };
    let (_, unused) = recorder();
    match slot.claim("show", unused) {
        Claim::Running(reply) => assert_eq!(reply, "ok"),
        other => panic!("the second launch found {other:?}"),
    }
    assert_eq!(slot.send("now").unwrap(), "playing\tGo");
    assert_eq!(*seen.lock().unwrap(), ["show", "now"]);
}

#[test]
fn a_declined_request_is_told_apart_from_a_silent_copy() {
    let (_dir, slot) = slot();
    let (seen, handle) = recorder();
    let Claim::First(_guard) = slot.claim("show", handle) else {
        panic!("the first launch is the running copy");
    };
    let declined = slot.send("frobnicate").unwrap_err();
    assert_eq!(declined.kind(), std::io::ErrorKind::PermissionDenied);
    // A launch is told at once, rather than waiting out ANSWER_WAIT.
    let (_, unused) = recorder();
    let started = Instant::now();
    assert!(matches!(slot.claim("frobnicate", unused), Claim::Declined));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(*seen.lock().unwrap(), ["frobnicate", "frobnicate"]);
    let two_lines = slot.send("show\nshow").unwrap_err();
    assert_eq!(two_lines.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn sending_with_nothing_running_says_so() {
    let (_dir, slot) = slot();
    let error = slot.send("show").unwrap_err();
    assert!(
        matches!(
            error.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
        ),
        "{error:?}"
    );
}

#[test]
fn a_scoped_slot_runs_beside_the_default() {
    let (_dir, slot) = slot();
    let demo = slot.clone().scoped("demo");
    assert_ne!(demo.dir(), slot.dir());
    let (_, first) = recorder();
    let (_, beside) = recorder();
    assert!(matches!(slot.claim("show", first), Claim::First(_)));
    assert!(matches!(demo.claim("show", beside), Claim::First(_)));
    // A scope cannot climb out of the slot.
    let sneaky = slot.clone().scoped("../../elsewhere");
    assert!(sneaky.dir().starts_with(slot.dir()), "{:?}", sneaky.dir());
}

#[test]
fn a_holder_that_does_not_answer_is_reported() {
    let (_dir, mut slot) = slot();
    slot.startup_wait = Duration::from_millis(300);
    // The lock is held, but nothing listens: a copy still starting, or stuck.
    let _held = lock(slot.dir()).unwrap().expect("the lock");
    let (_, handle) = recorder();
    assert!(matches!(slot.claim("show", handle), Claim::Unanswered));
}

#[test]
fn the_lock_is_free_again_once_its_holder_is_gone() {
    let (_dir, slot) = slot();
    let first = lock(slot.dir()).unwrap().expect("the first lock");
    assert!(lock(slot.dir()).unwrap().is_none());
    drop(first);
    assert!(lock(slot.dir()).unwrap().is_some());
}

#[test]
fn simultaneous_launches_make_one_running_copy() {
    for _ in 0..10 {
        let (_dir, slot) = slot();
        let barrier = Arc::new(Barrier::new(4));
        let launches: Vec<_> = (0..4)
            .map(|_| {
                let (slot, barrier) = (slot.clone(), Arc::clone(&barrier));
                std::thread::spawn(move || {
                    barrier.wait();
                    let (_, handle) = recorder();
                    slot.claim("ping", handle)
                })
            })
            .collect();
        let claims: Vec<Claim> = launches.into_iter().map(|l| l.join().unwrap()).collect();
        let first = claims
            .iter()
            .filter(|c| matches!(c, Claim::First(_)))
            .count();
        let running = claims
            .iter()
            .filter(|c| matches!(c, Claim::Running(_)))
            .count();
        assert_eq!((first, running), (1, 3));
    }
}

#[cfg(unix)]
#[test]
fn a_socket_left_by_a_crash_is_replaced_and_files_are_private() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, slot) = slot();
    std::fs::create_dir_all(slot.dir()).unwrap();
    std::fs::write(slot.dir().join(SOCKET_FILE), b"stale").unwrap();
    let (_, handle) = recorder();
    let Claim::First(_guard) = slot.claim("show", handle) else {
        panic!("a stale socket blocks nobody");
    };
    assert_eq!(slot.send("ping").unwrap(), "ok");
    let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(slot.dir().to_path_buf()), 0o700);
    assert_eq!(mode(slot.dir().join(LOCK_FILE)), 0o600);
    assert_eq!(mode(slot.dir().join(SOCKET_FILE)), 0o600);
}

#[test]
fn the_default_slot_is_per_app_and_scopes_stay_inside_it() {
    let one = Slot::new("rocks.example.One");
    let two = Slot::new("rocks.example.Two");
    assert_ne!(one.dir(), two.dir());
    assert!(one.clone().scoped("demo").dir().starts_with(one.dir()));
}

/// Serves loopback TCP with a token, as on Windows. Loopback only: nothing
/// leaves the machine.
fn tcp_server() -> (u16, String, Seen) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback port");
    let port = listener.local_addr().expect("a bound address").port();
    let token = new_token().expect("random bytes");
    let served = token.clone();
    let (seen, handle) = recorder();
    std::thread::spawn(move || serve(listener.incoming(), Some(&served), "test.app:", handle));
    (port, token, seen)
}

fn connect(port: u16) -> TcpStream {
    TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("a connection")
}

#[test]
fn tokens_are_random_and_compared_whole() {
    let token = new_token().unwrap();
    assert_eq!(token.len(), 64);
    assert_ne!(token, new_token().unwrap());
    assert!(token_matches(&token, token.as_bytes()));
    assert!(!token_matches(&token, &token.as_bytes()[..63]));
    assert!(!token_matches(&token, format!("{token}0").as_bytes()));
    assert!(!token_matches(&token, b""));
}

#[test]
fn loopback_requests_need_the_token_and_the_prefix() {
    let (port, token, seen) = tcp_server();
    let reply = exchange(connect(port), Some(&token), "test.app:", "show");
    assert_eq!(reply.unwrap(), "ok");
    let wrong = new_token().unwrap();
    assert!(exchange(connect(port), Some(&wrong), "test.app:", "show").is_err());
    assert!(exchange(connect(port), None, "test.app:", "show").is_err());
    let other = exchange(connect(port), Some(&token), "other.app:", "show").unwrap_err();
    assert_eq!(other.kind(), std::io::ErrorKind::InvalidData, "not ours");
    // Browsers reaching localhost send HTTP, which never carries the token.
    let mut browser = connect(port);
    browser
        .write_all(b"GET /test.app:show HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    let mut reply = Vec::new();
    browser.read_to_end(&mut reply).unwrap();
    assert!(reply.is_empty());
    assert_eq!(*seen.lock().unwrap(), ["show"]);
}

#[test]
fn oversized_and_stalled_clients_do_not_block_the_listener() {
    let (port, token, seen) = tcp_server();
    let mut flood = connect(port);
    // The listener stops reading at the limit and closes the connection, so
    // later writes may fail.
    let _ = flood.write_all(&vec![b'a'; REQUEST_LIMIT * 2]);
    // A client that sends part of a request and waits is dropped when its
    // time runs out.
    let mut stalled = connect(port);
    stalled.write_all(token.as_bytes()).unwrap();
    let started = Instant::now();
    let reply = exchange(connect(port), Some(&token), "test.app:", "ping");
    assert_eq!(reply.unwrap(), "ok");
    assert!(started.elapsed() < REQUEST_TIME * 3);
    let mut reply = Vec::new();
    stalled.read_to_end(&mut reply).unwrap();
    assert!(reply.is_empty());
    assert_eq!(*seen.lock().unwrap(), ["ping"]);
}

#[test]
fn a_long_link_fits_in_a_request() {
    let (port, token, seen) = tcp_server();
    let link = format!("open spotify:search:{}", "%E6%9D%B1".repeat(1000));
    assert!(link.len() > 8 * 1024);
    // Refused by the recorder, but it arrives whole.
    let _ = exchange(connect(port), Some(&token), "test.app:", &link);
    assert_eq!(*seen.lock().unwrap(), [link]);
}

#[test]
fn the_key_file_round_trips_and_stays_private() {
    let dir = tempfile::tempdir().unwrap();
    let token = new_token().unwrap();
    write_key(dir.path(), 4242, &token).unwrap();
    assert_eq!(read_key(dir.path()).unwrap(), (4242, token));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join(KEY_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[cfg(unix)]
#[test]
fn a_slot_that_cannot_listen_lets_the_lock_go() {
    // A socket path longer than the system allows (108 bytes on Linux, 104
    // on macOS) cannot be bound.
    let dir = tempfile::tempdir().unwrap();
    let deep = dir.path().join("d".repeat(120));
    let slot = Slot::at(&deep, "test.app");
    let (_, handle) = recorder();
    let Claim::First(_guard) = slot.claim("show", handle) else {
        panic!("the first launch runs");
    };
    // The lock is free, so a later launch runs too rather than waiting for
    // an answer that cannot come.
    assert!(lock(slot.dir()).unwrap().is_some());
}
