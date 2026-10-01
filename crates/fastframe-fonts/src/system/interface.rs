//! The face the platform draws its own interface with.
//!
//! - macOS: CoreText's system interface font (San Francisco, `SFNS.ttf`), a
//!   variable face; its optical size is held at the text cut, since the
//!   file's default is the display cut.
//! - Windows: Segoe UI Variable when installed (Windows 11), else the
//!   message font the display settings name (Segoe UI), else Segoe UI.
//! - Linux and other Unix: fontconfig's answer for `system-ui` at each
//!   weight, asked through `fc-match` as `fastframe-text` asks for hinting.
//!   The desktop's configuration decides: GNOME's Adwaita Sans, a user's
//!   own `sans-serif`, or whatever else it prefers.
//!   An `fc-match` that has not answered within a second is stopped, and
//!   Inter stays.
//!
//! Files are memory-mapped and kept for the life of the process, so only the
//! pages epaint reads are loaded. A face must draw Latin outlines, or Inter
//! stays.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use skrifa::MetadataProvider as _;

use super::{draws, faces, map};
use crate::Weight;

/// The optical size interface text is drawn at. San Francisco's axis starts
/// at 17 and defaults to its 28-point display cut; Inter's and Segoe UI
/// Variable's start lower. Clamped to each face's axis.
const TEXT_OPTICAL_SIZE: f32 = 14.0;

/// The platform's interface face, one per [`Weight`].
#[derive(Debug)]
pub(crate) struct Interface {
    /// The family, for the log.
    pub(crate) family: String,
    /// A face for each of [`Weight::ALL`], in that order.
    pub(crate) faces: Vec<Face>,
}

/// One weight of the interface face.
#[derive(Debug)]
pub(crate) struct Face {
    pub(crate) bytes: &'static [u8],
    pub(crate) index: u32,
    /// Variation coordinates: `wght` for the weight, `opsz` at the text cut.
    pub(crate) coords: Vec<([u8; 4], f32)>,
}

impl Interface {
    /// The face drawn for `weight`.
    pub(crate) fn face(&self, weight: Weight) -> &Face {
        let position = Weight::ALL
            .iter()
            .position(|each| *each == weight)
            .unwrap_or(0);
        &self.faces[position]
    }
}

/// The platform's interface face, found once per process (a CoreText call,
/// four `fc-match` runs, or a walk of the Windows font directories), or
/// `None` when it cannot be found or read.
pub(crate) fn interface() -> Option<&'static Interface> {
    static FACE: OnceLock<Option<Interface>> = OnceLock::new();
    FACE.get_or_init(|| {
        let started = std::time::Instant::now();
        let found = resolve();
        match &found {
            Some(interface) => log::info!(
                "interface font: {} ({:.1} ms)",
                interface.family,
                started.elapsed().as_secs_f32() * 1e3
            ),
            None => log::info!("no system interface font found; Inter stays"),
        }
        found
    })
    .as_ref()
}

/// A file and face chosen for one weight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Choice {
    pub(crate) path: PathBuf,
    pub(crate) index: u32,
}

/// Maps the chosen files and fixes each weight's coordinates. A weight whose
/// face cannot be read or draws no Latin takes the regular one's; without a
/// readable regular face there is no interface face.
pub(crate) fn assemble(family: String, choices: &[Choice]) -> Option<Interface> {
    let mut mapped: Vec<(PathBuf, &'static [u8])> = Vec::new();
    let mut bytes_of = |path: &Path| -> Option<&'static [u8]> {
        if let Some((_, bytes)) = mapped.iter().find(|(known, _)| known == path) {
            return Some(bytes);
        }
        // Kept for the process's life: epaint borrows the bytes for as long
        // as the fonts are installed, and every window installs them.
        let bytes: &'static [u8] = Box::leak(Box::new(map(path)?));
        mapped.push((path.to_path_buf(), bytes));
        Some(bytes)
    };
    let mut resolved: Vec<Option<Face>> = Vec::new();
    for (weight, choice) in Weight::ALL.iter().zip(choices) {
        let face = bytes_of(&choice.path).and_then(|bytes| face(bytes, choice.index, *weight));
        resolved.push(face);
    }
    let regular = resolved.first()?.as_ref()?;
    let (bytes, index) = (regular.bytes, regular.index);
    let faces = Weight::ALL
        .iter()
        .zip(resolved)
        .map(|(weight, face)| {
            face.unwrap_or_else(|| Face {
                bytes,
                index,
                coords: coords(bytes, index, *weight),
            })
        })
        .collect();
    Some(Interface { family, faces })
}

/// A face that draws Latin text upright, with its coordinates for `weight`.
fn face(bytes: &'static [u8], index: u32, weight: Weight) -> Option<Face> {
    let (_, font) = faces(bytes).into_iter().find(|(each, _)| *each == index)?;
    let latin = "aAgR".chars().all(|character| draws(&font, character));
    if !latin || font.attributes().style != skrifa::attribute::Style::Normal {
        return None;
    }
    Some(Face {
        bytes,
        index,
        coords: coords(bytes, index, weight),
    })
}

/// `wght` at the weight and `opsz` at the text cut, for the axes the face
/// has, each clamped to its range.
fn coords(bytes: &[u8], index: u32, weight: Weight) -> Vec<([u8; 4], f32)> {
    let Some((_, font)) = faces(bytes).into_iter().find(|(each, _)| *each == index) else {
        return Vec::new();
    };
    let mut coords = Vec::new();
    for axis in font.axes().iter() {
        let clamp = |value: f32| value.clamp(axis.min_value(), axis.max_value());
        match &axis.tag().to_be_bytes() {
            b"wght" => coords.push((*b"wght", clamp(weight.value()))),
            b"opsz" => coords.push((*b"opsz", clamp(TEXT_OPTICAL_SIZE))),
            _ => {}
        }
    }
    coords
}

/// Every upright face of the first of `families` installed under `dirs`,
/// with its weight and whether it varies in weight. Families match by their
/// typographic name, else their family name, ignoring case.
#[cfg_attr(
    not(windows),
    allow(
        dead_code,
        reason = "Windows looks faces up by family; the tests run everywhere"
    )
)]
pub(crate) fn family_faces(
    dirs: &[PathBuf],
    families: &[String],
) -> Option<(String, Vec<Candidate>)> {
    let mut found: Vec<(usize, Candidate)> = Vec::new();
    for dir in dirs {
        walk(dir, 0, families, &mut found);
    }
    let best = found.iter().map(|(family, _)| *family).min()?;
    let mut faces: Vec<Candidate> = found
        .into_iter()
        .filter(|(family, _)| *family == best)
        .map(|(_, candidate)| candidate)
        .collect();
    faces.sort_by(|one, other| (&one.path, one.index).cmp(&(&other.path, other.index)));
    faces.dedup();
    Some((families[best].clone(), faces))
}

/// An installed upright face of a wanted family.
#[cfg_attr(
    not(windows),
    allow(
        dead_code,
        reason = "Windows looks faces up by family; the tests run everywhere"
    )
)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Candidate {
    pub(crate) path: PathBuf,
    pub(crate) index: u32,
    pub(crate) weight: f32,
    pub(crate) variable: bool,
}

#[cfg_attr(
    not(windows),
    allow(
        dead_code,
        reason = "Windows looks faces up by family; the tests run everywhere"
    )
)]
fn walk(dir: &Path, depth: usize, families: &[String], found: &mut Vec<(usize, Candidate)>) {
    if depth >= super::FONT_SCAN_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() || (kind.is_symlink() && path.is_dir()) {
            walk(&path, depth + 1, families, found);
            continue;
        }
        if !super::is_font_file(&path) {
            continue;
        }
        let Some(data) = map(&path) else {
            continue;
        };
        for (index, font) in faces(&data) {
            let attributes = font.attributes();
            if attributes.style != skrifa::attribute::Style::Normal {
                continue;
            }
            let Some(family) = families
                .iter()
                .position(|family| super::named(&font, family))
            else {
                continue;
            };
            let variable = font
                .axes()
                .iter()
                .any(|axis| axis.tag().to_be_bytes() == *b"wght");
            found.push((
                family,
                Candidate {
                    path: path.clone(),
                    index,
                    weight: attributes.weight.value(),
                    variable,
                },
            ));
        }
    }
}

/// The face each of [`Weight::ALL`] draws with: a face that varies in
/// weight serves them all, else each takes the nearest static weight, ties
/// going to the heavier so a medium still stands out from regular.
#[cfg_attr(
    not(windows),
    allow(
        dead_code,
        reason = "Windows looks faces up by family; the tests run everywhere"
    )
)]
pub(crate) fn choose_weights(faces: &[Candidate]) -> Option<Vec<Choice>> {
    if let Some(variable) = faces.iter().find(|face| face.variable) {
        let choice = Choice {
            path: variable.path.clone(),
            index: variable.index,
        };
        return Some(vec![choice; Weight::ALL.len()]);
    }
    Weight::ALL
        .iter()
        .map(|weight| {
            faces
                .iter()
                .min_by(|one, other| {
                    let distance = |face: &Candidate| (face.weight - weight.value()).abs();
                    distance(one)
                        .total_cmp(&distance(other))
                        .then(other.weight.total_cmp(&one.weight))
                })
                .map(|face| Choice {
                    path: face.path.clone(),
                    index: face.index,
                })
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn resolve() -> Option<Interface> {
    const SFNS: &str = "/System/Library/Fonts/SFNS.ttf";
    let path = super::macos::interface_font().unwrap_or_else(|| PathBuf::from(SFNS));
    let choice = Choice { path, index: 0 };
    let data = map(&choice.path)?;
    let family = super::family_name(&data, 0).unwrap_or_else(|| "San Francisco".into());
    drop(data);
    assemble(family, &vec![choice; Weight::ALL.len()])
}

#[cfg(windows)]
fn resolve() -> Option<Interface> {
    let mut families = vec![
        "Segoe UI Variable".to_owned(),
        "Segoe UI Variable Text".to_owned(),
    ];
    if let Some(message) = super::windows::message_font() {
        families.push(message);
    }
    families.push("Segoe UI".to_owned());
    let (family, faces) = family_faces(&super::font_directories(), &families)?;
    assemble(family, &choose_weights(&faces)?)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn resolve() -> Option<Interface> {
    // The four questions run at once: about 7 ms rather than 26.
    let asking: Vec<Option<std::process::Child>> = Weight::ALL
        .iter()
        .map(|weight| {
            std::process::Command::new("fc-match")
                .args(["-f", FC_FORMAT, &fontconfig_pattern(*weight)])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok()
        })
        .collect();
    // One deadline for all four: they run side by side.
    let deadline = Instant::now() + FC_MATCH_LIMIT;
    let answers: Vec<Option<(String, Choice)>> = asking
        .into_iter()
        .map(|child| parse_fc_match(&printed(child?, deadline)?))
        .collect();
    let (family, regular) = answers.first()?.clone()?;
    // A weight fontconfig answers from another family (it has no bold, so
    // fontconfig reaches for the next family that does) takes the regular face.
    let choices: Vec<Choice> = answers
        .into_iter()
        .map(|answer| match answer {
            Some((other, choice)) if other == family => choice,
            _ => regular.clone(),
        })
        .collect();
    assemble(family, &choices)
}

/// How long the `fc-match` runs get, all four together. They answer in a
/// few milliseconds, but the caller is the app's startup, and a fontconfig
/// that is scanning its font folders again, or cannot reach one of them, can
/// take far longer.
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
const FC_MATCH_LIMIT: Duration = Duration::from_secs(1);

/// The pause between two looks at a process that has not ended.
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
const PAUSE: Duration = Duration::from_millis(1);

/// What `child` printed, when it has ended well by `deadline`. One that has
/// not ended is killed and reaped, so it neither keeps running nor stays a
/// zombie. Only the process itself is stopped: what a wrapper script in
/// `fc-match`'s place started is not followed.
///
/// The output is read once the process has ended, which is enough for the
/// one line `fc-match` prints: a process that filled the pipe would wait for
/// a reader, and be stopped at the deadline.
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn printed(mut child: std::process::Child, deadline: Instant) -> Option<String> {
    use std::io::Read as _;

    let status = match wait_until(deadline, || child.try_wait().transpose()) {
        Some(Ok(status)) => status,
        given_up => {
            match given_up {
                Some(Err(error)) => log::warn!("could not wait for fc-match: {error}"),
                _ => log::warn!("fc-match did not answer in time and was stopped"),
            }
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    if !status.success() {
        return None;
    }
    let mut output = Vec::new();
    child.stdout.take()?.read_to_end(&mut output).ok()?;
    Some(String::from_utf8_lossy(&output).into_owned())
}

/// Asks `finished` until it answers or `deadline` passes. It is asked at
/// least once, so what has already ended is never given up on.
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn wait_until<T>(deadline: Instant, mut finished: impl FnMut() -> Option<T>) -> Option<T> {
    loop {
        if let Some(answer) = finished() {
            return Some(answer);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(PAUSE);
    }
}

/// The `fc-match` format [`parse_fc_match`] reads.
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
const FC_FORMAT: &str = "%{index}|%{family[0]}|%{file}";

/// The `fc-match` pattern for `system-ui` at `weight`. The `-` is escaped:
/// unescaped, fontconfig reads `system-ui` as the family `system` at a size
/// `ui`, and answers with its default face whatever the desktop configures
/// for `system-ui`.
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn fontconfig_pattern(weight: Weight) -> String {
    format!(r"system\-ui:weight={}:slant=0", fontconfig_weight(weight))
}

/// fontconfig's weight scale for a [`Weight`].
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn fontconfig_weight(weight: Weight) -> u32 {
    match weight {
        Weight::Regular => 80,
        Weight::Medium => 100,
        Weight::SemiBold => 180,
        Weight::Bold => 200,
    }
}

/// The family and face in `fc-match` output in [`FC_FORMAT`]. A named
/// instance of a variable face sets the index's upper bits; the weight's
/// own coordinates are set from the axes instead.
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn parse_fc_match(output: &str) -> Option<(String, Choice)> {
    let mut fields = output.lines().next()?.splitn(3, '|');
    let index: u32 = fields.next()?.trim().parse().ok()?;
    let family = fields.next()?.trim();
    let file = fields.next()?.trim();
    if family.is_empty() || file.is_empty() {
        return None;
    }
    Some((
        family.to_owned(),
        Choice {
            path: PathBuf::from(file),
            index: index & 0xFFFF,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(name)
    }

    #[test]
    fn system_ui_is_asked_for_by_its_escaped_name() {
        assert_eq!(
            fontconfig_pattern(Weight::Regular),
            r"system\-ui:weight=80:slant=0"
        );
        assert_eq!(
            fontconfig_pattern(Weight::Bold),
            r"system\-ui:weight=200:slant=0"
        );
    }

    #[test]
    fn fontconfig_answers_are_read() {
        assert_eq!(
            parse_fc_match("327680|Adwaita Sans|/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf"),
            Some((
                "Adwaita Sans".to_owned(),
                Choice {
                    path: PathBuf::from("/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf"),
                    index: 0
                }
            ))
        );
        assert_eq!(parse_fc_match("0||/a.ttf"), None);
        assert_eq!(parse_fc_match("x|Family|/a.ttf"), None);
        assert_eq!(parse_fc_match(""), None);
        assert_eq!(
            [
                Weight::Regular,
                Weight::Medium,
                Weight::SemiBold,
                Weight::Bold
            ]
            .map(fontconfig_weight),
            [80, 100, 180, 200]
        );
    }

    #[test]
    fn waiting_ends_with_the_answer_or_at_the_deadline() {
        let far = Instant::now() + Duration::from_secs(60);
        let mut looks = 0;
        let answer = wait_until(far, || {
            looks += 1;
            (looks == 3).then_some("ended")
        });
        assert_eq!(answer, Some("ended"));
        assert_eq!(looks, 3);

        // No answer ever: it gives up, and not before the deadline.
        let started = Instant::now();
        let limit = Duration::from_millis(20);
        assert_eq!(wait_until(started + limit, || None::<()>), None);
        assert!(started.elapsed() >= limit);

        // A deadline already past still gets one look: the runs share one
        // deadline, and those that ended while another was waited for count.
        assert_eq!(wait_until(started, || Some("ended")), Some("ended"));
    }

    #[test]
    fn a_variable_face_serves_every_weight_at_its_coordinates() {
        let inter = fixture("fonts/InterVariable.ttf");
        let choices = vec![
            Choice {
                path: inter,
                index: 0
            };
            Weight::ALL.len()
        ];
        let interface = assemble("Inter".into(), &choices).expect("Inter draws Latin");
        assert_eq!(interface.faces.len(), 4);
        for weight in Weight::ALL {
            let face = interface.face(weight);
            assert!(
                face.coords.contains(&(*b"wght", weight.value())),
                "{weight:?}"
            );
            assert!(
                face.coords.contains(&(*b"opsz", TEXT_OPTICAL_SIZE)),
                "Inter's optical size starts at 14"
            );
        }
        // One mapping serves every weight.
        assert!(std::ptr::eq(
            interface.face(Weight::Regular).bytes,
            interface.face(Weight::Bold).bytes
        ));
    }

    #[test]
    fn a_face_that_cannot_draw_latin_is_refused() {
        // The Yi fixture has no Latin letters.
        let yi = fixture("tests/fixtures/yi/YiTest.ttf");
        let inter = fixture("fonts/InterVariable.ttf");
        let only_yi = vec![
            Choice {
                path: yi.clone(),
                index: 0
            };
            4
        ];
        assert!(assemble("Yi".into(), &only_yi).is_none());
        // A weight that cannot draw takes the regular face.
        let mut mixed = vec![
            Choice {
                path: inter,
                index: 0
            };
            4
        ];
        mixed[3] = Choice { path: yi, index: 0 };
        let interface = assemble("Inter".into(), &mixed).expect("regular is Inter");
        assert!(
            interface
                .face(Weight::Bold)
                .coords
                .contains(&(*b"wght", 700.0))
        );
        assert!(std::ptr::eq(
            interface.face(Weight::Regular).bytes,
            interface.face(Weight::Bold).bytes
        ));
        let missing = vec![
            Choice {
                path: PathBuf::from("/no/such/font.ttf"),
                index: 0
            };
            4
        ];
        assert!(assemble("None".into(), &missing).is_none());
    }

    #[test]
    fn static_weights_take_the_nearest_face_and_ties_go_heavier() {
        let face = |name: &str, weight: f32| Candidate {
            path: PathBuf::from(name),
            index: 0,
            weight,
            variable: false,
        };
        let segoe = [
            face("segoeui.ttf", 400.0),
            face("seguisb.ttf", 600.0),
            face("segoeuib.ttf", 700.0),
            face("segoeuil.ttf", 300.0),
        ];
        let paths: Vec<PathBuf> = choose_weights(&segoe)
            .expect("faces")
            .into_iter()
            .map(|choice| choice.path)
            .collect();
        assert_eq!(
            paths,
            ["segoeui.ttf", "seguisb.ttf", "seguisb.ttf", "segoeuib.ttf"]
                .map(PathBuf::from)
                .to_vec(),
            "500 is as far from 400 as from 600, and goes heavier"
        );
        let mut with_variable = segoe.to_vec();
        with_variable.push(Candidate {
            variable: true,
            ..face("SegUIVar.ttf", 400.0)
        });
        assert!(
            choose_weights(&with_variable)
                .expect("faces")
                .iter()
                .all(|choice| choice.path == Path::new("SegUIVar.ttf"))
        );
        assert!(choose_weights(&[]).is_none());
    }

    #[test]
    fn the_first_installed_family_is_found_by_name() {
        let dirs = vec![fixture("fonts"), fixture("tests/fixtures")];
        let wanted = [
            "Segoe UI Variable".to_owned(),
            "inter variable".to_owned(),
            "Inter".to_owned(),
        ];
        let (family, faces) = family_faces(&dirs, &wanted).expect("Inter is there");
        assert_eq!(family, "inter variable", "matched ignoring case, in order");
        assert_eq!(faces.len(), 1);
        assert!(faces[0].variable);
        assert!(family_faces(&dirs, &["Segoe UI".to_owned()]).is_none());
    }

    /// What this machine's desktop draws its interface with:
    /// `cargo test -p fastframe-fonts -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads this machine's fonts"]
    fn which_face_draws_the_interface_here() {
        let interface = interface().expect("an interface face");
        println!("{}", interface.family);
        for weight in Weight::ALL {
            let face = interface.face(weight);
            println!("{weight:?}: face {} {:?}", face.index, face.coords);
        }
    }

    /// Windows 10's interface face, which Windows 11 still installs: its
    /// static Segoe UI files, found by family and chosen by weight.
    #[cfg(windows)]
    #[test]
    #[ignore = "reads this machine's fonts"]
    fn segoe_ui_is_found_by_weight_here() {
        println!("message font: {:?}", super::super::windows::message_font());
        let (family, faces) =
            family_faces(&super::super::font_directories(), &["Segoe UI".to_owned()])
                .expect("Segoe UI is installed");
        assert_eq!(family, "Segoe UI");
        assert!(faces.iter().all(|face| !face.variable));
        let files: Vec<String> = choose_weights(&faces)
            .expect("faces")
            .iter()
            .map(|choice| {
                let name = choice.path.file_name().unwrap_or_default();
                name.to_string_lossy().to_lowercase()
            })
            .collect();
        assert_eq!(
            files,
            ["segoeui.ttf", "seguisb.ttf", "seguisb.ttf", "segoeuib.ttf"]
        );
    }
}
