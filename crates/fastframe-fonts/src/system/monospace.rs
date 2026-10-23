//! The face the platform draws fixed-pitch text with.
//!
//! - macOS: CoreText's user fixed-pitch font, which Terminal's and TextEdit's
//!   defaults follow: Menlo, unless the user chose another. Menlo is a
//!   collection, so its regular face is picked from the file.
//! - Windows: Cascadia Mono when installed (Windows 11), else Consolas.
//! - Linux and other Unix: fontconfig's answer for `monospace`, asked through
//!   `fc-match` as the interface face is. The desktop's configuration
//!   decides: the terminal font Omarchy sets, DejaVu Sans Mono, or whatever
//!   else it prefers. An `fc-match` that has not answered within a second is
//!   stopped.
//!
//! Only the regular weight is read: egui has one monospace family. A face
//! must draw Latin outlines upright, or egui's own monospace stays.

use std::sync::OnceLock;

use super::interface::Interface;
use crate::Weight;

/// The platform's monospace face at regular weight, found once per process,
/// or `None` when it cannot be found or read.
pub(crate) fn monospace() -> Option<&'static Interface> {
    static FACE: OnceLock<Option<Interface>> = OnceLock::new();
    FACE.get_or_init(|| {
        let started = std::time::Instant::now();
        let found = resolve();
        match &found {
            Some(face) => log::info!(
                "monospace font: {} ({:.1} ms)",
                face.family,
                started.elapsed().as_secs_f32() * 1e3
            ),
            None => log::info!("no system monospace font found; egui's stays"),
        }
        found
    })
    .as_ref()
}

/// The upright face of a collection nearest to regular weight, from each
/// face's index, weight and whether it is upright; ties go to the lower
/// index, as the collection lists it.
#[cfg_attr(
    not(target_os = "macos"),
    allow(
        dead_code,
        reason = "only CoreText names a collection without its face; the tests run everywhere"
    )
)]
fn regular_face(faces: &[(u32, f32, bool)]) -> Option<u32> {
    faces
        .iter()
        .filter(|(_, _, upright)| *upright)
        .min_by(|one, other| {
            let distance = |weight: f32| (weight - Weight::Regular.value()).abs();
            distance(one.1)
                .total_cmp(&distance(other.1))
                .then(one.0.cmp(&other.0))
        })
        .map(|(index, _, _)| *index)
}

/// The `fc-match` pattern for `monospace` at regular weight, upright.
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn fontconfig_pattern() -> String {
    format!(
        "monospace:weight={}:slant=0",
        super::interface::fontconfig_weight(Weight::Regular)
    )
}

#[cfg(target_os = "macos")]
fn resolve() -> Option<Interface> {
    let path = super::macos::fixed_pitch_font()?;
    let data = super::map(&path)?;
    let faces: Vec<(u32, f32, bool)> = super::faces(&data)
        .into_iter()
        .map(|(index, font)| {
            let attributes = skrifa::MetadataProvider::attributes(&font);
            (
                index,
                attributes.weight.value(),
                attributes.style == skrifa::attribute::Style::Normal,
            )
        })
        .collect();
    let index = regular_face(&faces)?;
    let family = super::family_name(&data, index).unwrap_or_else(|| "Menlo".into());
    drop(data);
    super::interface::assemble(family, &[super::interface::Choice { path, index }])
}

#[cfg(windows)]
fn resolve() -> Option<Interface> {
    let families = ["Cascadia Mono".to_owned(), "Consolas".to_owned()];
    let (family, faces) = super::interface::family_faces(&super::font_directories(), &families)?;
    let choices = super::interface::choose_weights(&faces)?;
    super::interface::assemble(family, &choices[..1])
}

#[cfg(not(any(target_os = "macos", windows)))]
fn resolve() -> Option<Interface> {
    use super::interface::{FC_FORMAT, FC_MATCH_LIMIT, parse_fc_match, printed};

    let child = std::process::Command::new("fc-match")
        .args(["-f", FC_FORMAT, &fontconfig_pattern()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + FC_MATCH_LIMIT;
    let (family, choice) = parse_fc_match(&printed(child, deadline)?)?;
    super::interface::assemble(family, &[choice])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Menlo's collection: regular, bold, italic and bold italic.
    #[test]
    fn a_collections_regular_upright_face_is_chosen() {
        let menlo = [
            (0, 400.0, true),
            (1, 700.0, true),
            (2, 400.0, false),
            (3, 700.0, false),
        ];
        assert_eq!(regular_face(&menlo), Some(0));
        let bold_first = [(0, 700.0, true), (1, 400.0, false), (2, 400.0, true)];
        assert_eq!(regular_face(&bold_first), Some(2));
        assert_eq!(regular_face(&[(0, 300.0, true), (1, 500.0, true)]), Some(0));
        assert_eq!(regular_face(&[(0, 400.0, false)]), None, "no upright face");
    }

    /// Escaped like `system\-ui` it need not be: `monospace` has no `-`.
    #[test]
    fn fontconfig_is_asked_for_an_upright_regular_monospace() {
        assert_eq!(fontconfig_pattern(), "monospace:weight=80:slant=0");
    }
}
