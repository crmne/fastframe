//! Validated, owned interface fonts, extracted from ZapFast's custom font work.

use std::fmt;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use egui::{FontData, FontDefinitions};
use skrifa::MetadataProvider as _;

use crate::{Weight, system};

/// Largest accepted local font file (32 MiB), including a whole collection.
pub const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;

/// Why a local font could not be used. The app chooses how to present it.
#[derive(Debug)]
pub enum FontError {
    /// Reading the file failed.
    Io(std::io::Error),
    /// The path is not a regular file.
    NotAFile,
    /// The file exceeds [`MAX_FONT_BYTES`].
    TooLarge,
    /// The file or selected collection index is invalid.
    InvalidFace,
    /// The face has no usable Latin outlines for an interface.
    UnsupportedFace,
}

impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "Cannot read the font: {error}"),
            Self::NotAFile => f.write_str("The font must be a regular file."),
            Self::TooLarge => f.write_str("The font exceeds 32 MiB."),
            Self::InvalidFace => f.write_str("The font or collection face index is invalid."),
            Self::UnsupportedFace => f.write_str("The font needs rasterizable Latin outlines."),
        }
    }
}

impl std::error::Error for FontError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

/// An immutable local face ready to lead the interface's weight families.
///
/// Accepts TrueType/OpenType outlines in TTF, OTF, TTC and OTC containers.
/// The explicit index selects a collection face (zero for a single font).
/// A variable `wght` axis supplies the requested weights; a static face is
/// shared at every weight without synthesizing bold. Emoji and monospace
/// families stay as the app configured them. File ownership stays with the
/// app: this type never copies to a settings folder or deletes a file.
///
/// egui owns each weight's bytes, so a variable face can use four times the
/// file size. Definitions reuse those allocations through `Arc`.
#[derive(Clone, Debug)]
pub struct LocalFont {
    faces: [Arc<FontData>; 4],
    pub(crate) baseline: f32,
}

impl LocalFont {
    /// Reads at most [`MAX_FONT_BYTES`] plus one byte, then validates the
    /// selected face. Call off the interface thread. Later definitions do
    /// not reread the file, so removing or changing it cannot change a face.
    ///
    /// # Errors
    /// Returns [`FontError`] for unreadable, oversized or unsupported files.
    pub fn load(path: impl AsRef<Path>, index: u32) -> Result<Self, FontError> {
        let path = path.as_ref();
        let metadata = std::fs::metadata(path).map_err(FontError::Io)?;
        if !metadata.is_file() {
            return Err(FontError::NotAFile);
        }
        if metadata.len() > MAX_FONT_BYTES as u64 {
            return Err(FontError::TooLarge);
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(FontError::Io)?
            .take(MAX_FONT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(FontError::Io)?;
        Self::from_bytes(bytes, index)
    }

    /// Validates owned bytes without touching the filesystem.
    ///
    /// # Errors
    /// Returns [`FontError`] if the file is oversized, the selected face is
    /// invalid, or it cannot draw Latin letters and digits as outlines.
    pub fn from_bytes(bytes: Vec<u8>, index: u32) -> Result<Self, FontError> {
        if bytes.len() > MAX_FONT_BYTES {
            return Err(FontError::TooLarge);
        }
        let font =
            skrifa::FontRef::from_index(&bytes, index).map_err(|_| FontError::InvalidFace)?;
        if !['A', 'a', '0'].iter().all(|ch| system::draws(&font, *ch)) {
            return Err(FontError::UnsupportedFace);
        }
        let variable = font
            .axes()
            .iter()
            .any(|axis| axis.tag() == skrifa::Tag::new(b"wght"));
        let baseline = system::baseline_center(&bytes, index).ok_or(FontError::UnsupportedFace)?;
        let mut data = FontData::from_owned(bytes);
        data.index = index;
        let data = Arc::new(data);
        let faces = std::array::from_fn(|i| {
            if variable {
                let mut face = (*data).clone();
                face.tweak.coords =
                    egui::epaint::text::VariationCoords::new([(b"wght", Weight::ALL[i].value())]);
                Arc::new(face)
            } else {
                Arc::clone(&data)
            }
        });
        Ok(Self { faces, baseline })
    }

    pub(crate) fn lead(&self, fonts: &mut FontDefinitions) {
        for (weight, data) in Weight::ALL.into_iter().zip(&self.faces) {
            if let Some(family) = fonts.families.get_mut(&weight.family()) {
                let key = format!("fastframe-local-{}", weight.name());
                family.insert(0, key.clone());
                fonts.font_data.insert(key, Arc::clone(data));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontSetup, Monospace};
    fn static_bytes() -> Vec<u8> {
        egui::FontDefinitions::default().font_data["Ubuntu-Light"]
            .font
            .to_vec()
    }
    #[test]
    fn static_faces_share_storage_and_leave_monospace_and_fallbacks_alone() {
        let setup = FontSetup::default()
            .system_fallbacks(false)
            .weights(&[Weight::Bold])
            .monospace(Monospace::Inter);
        let before = setup.definitions();
        let local = LocalFont::from_bytes(static_bytes(), 0).unwrap();
        assert!(Arc::ptr_eq(&local.faces[0], &local.faces[3]));
        let after = setup.local_font(local).definitions();
        assert_eq!(
            before.families[&egui::FontFamily::Monospace],
            after.families[&egui::FontFamily::Monospace]
        );
        for weight in [Weight::Regular, Weight::Bold] {
            assert_eq!(
                &after.families[&weight.family()][1..],
                &before.families[&weight.family()]
            );
        }
        assert!(!after.families.contains_key(&Weight::Medium.family()));
    }
    #[test]
    fn variable_fonts_keep_each_weight_and_reuse_data() {
        let local = LocalFont::from_bytes(include_bytes!("../fonts/InterVariable.ttf").to_vec(), 0)
            .unwrap();
        assert!(!Arc::ptr_eq(&local.faces[0], &local.faces[3]));
        for (face, weight) in local.faces.iter().zip(Weight::ALL) {
            assert_eq!(
                face.tweak.coords,
                egui::epaint::text::VariationCoords::new([(b"wght", weight.value())])
            );
        }
        let setup = FontSetup::default()
            .system_fallbacks(false)
            .local_font(local);
        let a = setup.definitions();
        let b = setup.definitions();
        assert!(Arc::ptr_eq(
            &a.font_data["fastframe-local-inter"],
            &b.font_data["fastframe-local-inter"]
        ));
    }
    #[test]
    fn bad_data_indexes_and_non_interface_faces_are_rejected() {
        assert!(matches!(
            LocalFont::from_bytes(b"not a font".to_vec(), 0),
            Err(FontError::InvalidFace)
        ));
        assert!(matches!(
            LocalFont::from_bytes(static_bytes(), 1),
            Err(FontError::InvalidFace)
        ));
        assert!(matches!(
            LocalFont::from_bytes(
                include_bytes!("../tests/fixtures/yi/YiTest.ttf").to_vec(),
                0
            ),
            Err(FontError::UnsupportedFace)
        ));
        assert!(matches!(
            LocalFont::from_bytes(vec![0; MAX_FONT_BYTES + 1], 0),
            Err(FontError::TooLarge)
        ));
    }
    #[test]
    fn a_collection_uses_the_requested_face_index() {
        let mut font = static_bytes();
        let count = u16::from_be_bytes([font[4], font[5]]) as usize;
        for i in 0..count {
            let start = 12 + i * 16 + 8;
            let offset = u32::from_be_bytes(font[start..start + 4].try_into().unwrap());
            font[start..start + 4].copy_from_slice(&(offset + 16).to_be_bytes());
        }
        let mut collection = b"ttcf".to_vec();
        for value in [0x0001_0000_u32, 1, 16] {
            collection.extend(value.to_be_bytes());
        }
        collection.extend(font);
        assert!(LocalFont::from_bytes(collection.clone(), 0).is_ok());
        assert!(matches!(
            LocalFont::from_bytes(collection, 1),
            Err(FontError::InvalidFace)
        ));
    }

    #[test]
    fn local_baselines_leave_monospace_fallback_metrics_unchanged() {
        let fallback = system::Fallback {
            name: "fallback-yi".into(),
            bytes: include_bytes!("../tests/fixtures/yi/YiTest.ttf"),
            index: 0,
            scale: 1.0,
            y_offset_factor: 0.25,
        };
        let fallbacks = Box::leak(Box::new([fallback]));
        let setup = FontSetup::default();
        let before = setup.definitions_with(fallbacks, None, None);
        let after = setup
            .local_font(LocalFont::from_bytes(static_bytes(), 0).unwrap())
            .definitions_with(fallbacks, None, None);
        assert_eq!(
            after.font_data["fallback-yi"].tweak,
            before.font_data["fallback-yi"].tweak
        );
        assert_eq!(
            after.families[&egui::FontFamily::Monospace],
            before.families[&egui::FontFamily::Monospace]
        );
        assert!(
            after.families[&Weight::Regular.family()]
                .contains(&"fastframe-local-fallback-yi".into())
        );
    }

    #[test]
    fn loading_is_bounded_and_the_loaded_face_outlives_its_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            LocalFont::load(dir.path(), 0),
            Err(FontError::NotAFile)
        ));
        let path = dir.path().join("font.ttf");
        std::fs::write(&path, static_bytes()).unwrap();
        let local = LocalFont::load(&path, 0).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(LocalFont::load(&path, 0), Err(FontError::Io(_))));
        assert!(
            FontSetup::default()
                .system_fallbacks(false)
                .local_font(local)
                .definitions()
                .font_data
                .contains_key("fastframe-local-inter")
        );
        std::fs::File::create(&path)
            .unwrap()
            .set_len(MAX_FONT_BYTES as u64 + 1)
            .unwrap();
        assert!(matches!(
            LocalFont::load(&path, 0),
            Err(FontError::TooLarge)
        ));
    }
}
