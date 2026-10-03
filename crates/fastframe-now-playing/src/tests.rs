use super::*;

fn playing() -> State {
    State {
        playback: Playback::Playing,
        track: Some(Track {
            id: "spotify:track:1".into(),
            title: "Go".into(),
            ..Track::default()
        }),
        volume: Some(0.75),
        ..State::default()
    }
}

#[test]
fn a_change_goes_out_at_once_and_the_position_once_a_second() {
    let mut throttle = Throttle::default();
    let start = Instant::now();
    let state = playing();
    assert_eq!(throttle.due(&state, start), Some(false), "the first state");
    throttle.sent(state.clone());
    throttle.last_sent = Some(start);

    let moved = State {
        position: Duration::from_millis(300),
        ..state.clone()
    };
    assert_eq!(
        throttle.due(&moved, start + Duration::from_millis(300)),
        None
    );
    assert_eq!(
        throttle.due(&moved, start + POSITION_INTERVAL),
        Some(false),
        "the position, a second later"
    );

    let paused = State {
        playback: Playback::Paused,
        ..moved.clone()
    };
    assert_eq!(
        throttle.due(&paused, start + Duration::from_millis(310)),
        Some(false),
        "a pause, at once"
    );
    assert_eq!(throttle.due(&state, start), None, "nothing new");
}

#[test]
fn a_volume_request_republishes_the_level_even_when_unchanged() {
    let mut throttle = Throttle::default();
    let state = playing();
    throttle.due(&state, Instant::now());
    throttle.sent(state.clone());
    assert_eq!(throttle.due(&state, Instant::now()), None);
    // A client asked for a level the app already had, or rounded to it: the
    // bus holds the client's copy until it is told again.
    throttle.volume_requested.set(true);
    assert_eq!(throttle.due(&state, Instant::now()), Some(true));
    throttle.sent(state.clone());
    assert_eq!(throttle.due(&state, Instant::now()), None);
}

#[test]
fn artwork_arriving_is_a_change_worth_sending() {
    let bare = playing();
    let mut with_art = bare.clone();
    if let Some(track) = &mut with_art.track {
        track.art_file = Some(PathBuf::from("/home/ada/.cache/art/0badc0de"));
    }
    assert!(!same_but_position(&bare, &with_art));
}

#[test]
fn an_artwork_url_escapes_what_a_url_would_read() {
    assert_eq!(
        file_url(Path::new("/Users/ada #1/Caches/art/0badc0de"), true),
        "file:///Users/ada%20%231/Caches/art/0badc0de"
    );
    assert_eq!(
        file_url(Path::new("/a-b/c.d/e_f~g/0badc0de"), true),
        "file:///a-b/c.d/e_f~g/0badc0de"
    );
    // Windows opens what follows `file://` as written.
    assert_eq!(
        file_url(Path::new(r"C:\Users\ada #1\art\0badc0de"), false),
        r"file://C:\Users\ada #1\art\0badc0de"
    );
}

#[test]
fn the_desktop_entry_is_the_flatpak_id_inside_a_sandbox() {
    assert_eq!(
        desktop_entry_for(Some("com.getsolco.Solco"), "solco"),
        "com.getsolco.Solco"
    );
    assert_eq!(desktop_entry_for(None, "solco"), "solco");
    assert_eq!(desktop_entry_for(Some(""), "solco"), "solco");
}

#[cfg(target_os = "linux")]
mod mpris {
    use super::*;
    use crate::mpris::{metadata, track_id_from_path, track_path, track_path_prefix};

    #[test]
    fn any_track_id_survives_the_bus() {
        let prefix = track_path_prefix("spotifast");
        assert_eq!(prefix, "/spotifast/Track/");
        for id in ["spotify:track:14XWXWv5FoCbFzLksawpEe", "42", "東京 #1", ""] {
            let path = track_path(&prefix, id).expect("an object path");
            assert_eq!(
                track_id_from_path(&prefix, path.as_str()).as_deref(),
                Some(id)
            );
        }
        assert_eq!(track_id_from_path(&prefix, "/other/Track/t41"), None);
        assert_eq!(track_id_from_path(&prefix, "/spotifast/Track/t4"), None);
        assert_eq!(
            track_path_prefix("com.getsolco-Solco"),
            "/com_getsolco_Solco/Track/"
        );
    }

    #[test]
    fn the_metadata_carries_the_artwork_file_and_everything_solco_knows() {
        let prefix = track_path_prefix("solco");
        let track = Track {
            id: "7".into(),
            title: "Strings of Life".into(),
            artists: vec!["Rhythim Is Rhythim".into()],
            album: "Strings of Life".into(),
            duration: Some(Duration::from_secs(400)),
            art_file: Some(PathBuf::from("/home/ada #1/.cache/solco/art/7.jpg")),
            art_url: Some("https://example.com/7.jpg".into()),
            genres: vec!["Techno".into()],
            bpm: Some(124.6),
            rating: Some(1.4),
            ..Track::default()
        };
        let metadata = metadata(Some(&track), &prefix);
        assert_eq!(metadata.title(), Some("Strings of Life"));
        assert_eq!(
            metadata.art_url().as_deref(),
            Some("file:///home/ada%20%231/.cache/solco/art/7.jpg"),
            "the file wins over the web address"
        );
        assert_eq!(metadata.length().map(|t| t.as_millis()), Some(400_000));
        assert_eq!(metadata.genre(), Some(vec!["Techno".to_owned()]));
        assert_eq!(metadata.audio_bpm(), Some(125));
        assert_eq!(metadata.user_rating(), Some(1.0));
        let id = metadata.trackid().expect("a track id");
        assert_eq!(
            track_id_from_path(&prefix, id.as_str()).as_deref(),
            Some("7")
        );

        let web_only = Track {
            art_file: None,
            ..track
        };
        assert_eq!(
            metadata_art(&web_only, &prefix).as_deref(),
            Some("https://example.com/7.jpg")
        );
    }

    fn metadata_art(track: &Track, prefix: &str) -> Option<String> {
        metadata(Some(track), prefix)
            .art_url()
            .map(|url| url.to_string())
    }

    #[test]
    fn no_track_is_empty_metadata() {
        assert!(metadata(None, "/x/Track/").title().is_none());
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod native {
    use souvlaki::{MediaControlEvent, MediaPosition, SeekDirection};

    use super::*;
    use crate::native::{command_for, should_claim};

    #[test]
    fn control_events_become_commands() {
        assert_eq!(
            command_for(MediaControlEvent::Toggle, ""),
            Command::PlayPause
        );
        assert_eq!(
            command_for(MediaControlEvent::Seek(SeekDirection::Backward), ""),
            Command::SeekBy(-SEEK_STEP_MS)
        );
        assert_eq!(
            command_for(
                MediaControlEvent::SeekBy(SeekDirection::Forward, Duration::from_secs(5)),
                ""
            ),
            Command::SeekBy(5_000)
        );
        assert_eq!(
            command_for(
                MediaControlEvent::SetPosition(MediaPosition(Duration::from_secs(30))),
                "track-1"
            ),
            Command::SetPosition {
                track_id: "track-1".into(),
                position: Duration::from_secs(30),
            }
        );
        assert_eq!(
            command_for(MediaControlEvent::SetVolume(1.5), ""),
            Command::SetVolume(1.0)
        );
    }

    #[test]
    fn a_remembered_paused_track_claims_the_media_keys_once() {
        let mut state = State {
            playback: Playback::Paused,
            ..playing()
        };
        assert!(should_claim(false, &state));
        assert!(!should_claim(true, &state));
        state.track = None;
        assert!(!should_claim(false, &state));
    }
}
