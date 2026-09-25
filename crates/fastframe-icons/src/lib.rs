//! Embedded SVG icons for egui apps.
//!
//! An app lists its icons once with [`icons!`], which generates the `Icon`
//! enum, each icon's URI and bytes, and an `image` helper. [`install`] serves
//! them to egui:
//!
//! ```
//! fastframe_icons::icons! {
//!     /// Every icon the interface draws.
//!     pub enum Icon {
//!         prefix: "myapp-icon-",
//!         directory: "../assets/icons/",
//!         // An app file: `Archive => "archive"` embeds ../assets/icons/archive.svg.
//!         Check => lucide "check", // one of the shared icons in `lucide`
//!         Close => lucide "x",
//!     }
//! }
//!
//! let ctx = egui::Context::default();
//! fastframe_icons::install::<Icon>(&ctx);
//! let image = Icon::Check.image(egui::Color32::WHITE, 16.0);
//! # let _ = image;
//! ```
//!
//! `directory` is relative to the file that invokes the macro, as with
//! `include_bytes!`. `lucide "name"` takes one of the shared icons in
//! [`lucide`] instead of a file of the app's own.
//!
//! Widgets that draw icons (buttons, hover tints, sizes) stay in the app.

use std::marker::PhantomData;
use std::sync::Arc;

pub mod lucide;

/// The egui this crate was built with, for the code [`icons!`] generates.
#[doc(hidden)]
pub use egui;

/// A set of embedded icons, implemented by the enum [`icons!`] generates.
pub trait IconSet: Copy + Eq + std::fmt::Debug + Send + Sync + 'static {
    /// Every icon in the set, in declaration order.
    fn all() -> &'static [Self];
    /// The URI egui loads the icon by: `bytes://<prefix><name>.svg`.
    fn uri(self) -> &'static str;
    /// The icon's SVG.
    fn bytes(self) -> &'static [u8];
}

/// Serves an [`IconSet`] to egui for the life of the context.
///
/// Call it once per context, after installing egui's image loaders (for
/// example `egui_extras::install_image_loaders`, with the `svg` feature),
/// which turn the bytes into textures. Installing the same set twice is
/// harmless; the first loader answers.
pub fn install<I: IconSet>(ctx: &egui::Context) {
    ctx.add_bytes_loader(Arc::new(Loader::<I>(PhantomData)));
}

/// An icon image at `size` points square, tinted `tint`.
///
/// The generated `Icon::image` calls this. Apps' SVGs are drawn in white so
/// the tint is the colour on screen.
pub fn image(uri: &'static str, tint: egui::Color32, size: f32) -> egui::Image<'static> {
    egui::Image::new(uri)
        .tint(tint)
        .fit_to_exact_size(egui::Vec2::splat(size))
}

/// The bytes loader behind [`install`].
///
/// egui's own `include_bytes` loader forgets an image's bytes when its
/// texture is uploaded and `reduce_texture_memory` is on. An icon drawn at a
/// second size then finds neither bytes nor texture once egui prunes the
/// extra size, and paints the red "failed" placeholder. This loader never
/// forgets: icons are small, so holding them costs nothing next to a photo.
struct Loader<I>(PhantomData<fn() -> I>);

impl<I: IconSet> egui::load::BytesLoader for Loader<I> {
    fn id(&self) -> &str {
        // One loader per set, so two sets never shadow each other's id.
        std::any::type_name::<Self>()
    }

    fn load(&self, _: &egui::Context, uri: &str) -> egui::load::BytesLoadResult {
        match I::all().iter().find(|icon| icon.uri() == uri) {
            Some(icon) => Ok(egui::load::BytesPoll::Ready {
                size: None,
                bytes: icon.bytes().into(),
                mime: Some("image/svg+xml".to_owned()),
            }),
            None => Err(egui::load::LoadError::NotSupported),
        }
    }

    fn forget(&self, _uri: &str) {}

    fn forget_all(&self) {}

    fn byte_size(&self) -> usize {
        I::all().iter().map(|icon| icon.bytes().len()).sum()
    }
}

/// Declares an app's icons: an enum, each icon's URI and SVG bytes, and an
/// [`IconSet`] implementation for [`install`].
///
/// ```ignore
/// fastframe_icons::icons! {
///     /// Every icon the interface draws.
///     #[derive(PartialOrd, Ord)]
///     pub enum Icon {
///         prefix: "spotifast-icon-",
///         directory: "../assets/icons/",
///         /// The play button.
///         Play => "play",
///         Search => lucide "search",
///     }
/// }
/// ```
///
/// The enum derives `Clone, Copy, Debug, Eq, PartialEq, Hash`; further
/// attributes pass through. It gets:
///
/// - `Icon::ALL`, every icon in declaration order;
/// - `const fn uri(self)`: `bytes://<prefix><file>.svg`, or
///   `bytes://<prefix>lucide-<name>.svg` for a shared icon;
/// - `const fn bytes(self)`: the SVG, embedded with `include_bytes!` from
///   `<directory><file>.svg` relative to the invoking file;
/// - `fn image(self, tint, size)`: an [`egui::Image`] at `size` points.
///
/// Keep the prefix unique to the app, so its URIs never collide with
/// another loader's.
#[macro_export]
macro_rules! icons {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            prefix: $prefix:literal,
            directory: $directory:literal,
            $(
                $(#[$variant_meta:meta])*
                $variant:ident => $($shared:ident)? $file:literal
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
        $vis enum $name {
            $($(#[$variant_meta])* $variant,)*
        }

        impl $name {
            /// Every icon, in declaration order.
            $vis const ALL: &'static [Self] = &[$(Self::$variant,)*];

            /// The URI egui loads this icon by.
            #[must_use]
            $vis const fn uri(self) -> &'static str {
                match self {
                    $(Self::$variant => concat!(
                        "bytes://", $prefix, $(stringify!($shared), "-",)? $file, ".svg"
                    ),)*
                }
            }

            /// The icon's SVG.
            #[must_use]
            $vis const fn bytes(self) -> &'static [u8] {
                match self {
                    $(Self::$variant => $crate::__icon_bytes!($directory, $($shared)? $file),)*
                }
            }

            /// The icon at `size` points square, tinted `tint`.
            $vis fn image(
                self,
                tint: $crate::egui::Color32,
                size: f32,
            ) -> $crate::egui::Image<'static> {
                $crate::image(self.uri(), tint, size)
            }
        }

        impl $crate::IconSet for $name {
            fn all() -> &'static [Self] {
                Self::ALL
            }

            fn uri(self) -> &'static str {
                $name::uri(self)
            }

            fn bytes(self) -> &'static [u8] {
                $name::bytes(self)
            }
        }
    };
}

/// One icon's bytes for [`icons!`]: an app file, or a shared Lucide icon
/// looked up at compile time.
#[doc(hidden)]
#[macro_export]
macro_rules! __icon_bytes {
    ($directory:literal, $file:literal) => {{
        let bytes: &'static [u8] = include_bytes!(concat!($directory, $file, ".svg"));
        bytes
    }};
    ($directory:literal, lucide $file:literal) => {{
        const BYTES: &[u8] = $crate::lucide::get($file);
        BYTES
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::load::{BytesLoader as _, BytesPoll};

    icons! {
        /// A set drawn from this crate's own files.
        enum Sample {
            prefix: "fastframe-test-",
            directory: "../tests/fixtures/",
            /// A file of the app's own.
            Dot => "dot",
            Close => lucide "x",
            Search => lucide "search",
        }
    }

    #[test]
    fn uris_name_the_app_and_the_source() {
        assert_eq!(Sample::Dot.uri(), "bytes://fastframe-test-dot.svg");
        assert_eq!(Sample::Close.uri(), "bytes://fastframe-test-lucide-x.svg");
        assert_eq!(Sample::ALL, [Sample::Dot, Sample::Close, Sample::Search]);
    }

    #[test]
    fn bytes_come_from_the_directory_or_the_shared_set() {
        assert_eq!(
            Sample::Dot.bytes(),
            include_bytes!("../tests/fixtures/dot.svg")
        );
        assert_eq!(Sample::Close.bytes(), lucide::get("x"));
        assert_eq!(Sample::Search.bytes(), lucide::get("search"));
    }

    #[test]
    fn the_loader_serves_every_icon_and_never_forgets() {
        let ctx = egui::Context::default();
        let loader = Loader::<Sample>(PhantomData);
        for icon in Sample::ALL {
            loader.forget(icon.uri());
            loader.forget_all();
            match loader.load(&ctx, icon.uri()) {
                Ok(BytesPoll::Ready { bytes, mime, .. }) => {
                    assert_eq!(&*bytes, icon.bytes());
                    assert_eq!(mime.as_deref(), Some("image/svg+xml"));
                }
                _ => panic!("{icon:?} was not served"),
            }
        }
        assert!(matches!(
            loader.load(&ctx, "bytes://someone-else.svg"),
            Err(egui::load::LoadError::NotSupported)
        ));
        let total: usize = Sample::ALL.iter().map(|icon| icon.bytes().len()).sum();
        assert_eq!(loader.byte_size(), total);
    }

    #[test]
    fn installing_registers_a_loader_egui_asks() {
        let ctx = egui::Context::default();
        install::<Sample>(&ctx);
        let loaded = ctx.try_load_bytes(Sample::Dot.uri());
        assert!(matches!(loaded, Ok(BytesPoll::Ready { .. })));
    }

    #[test]
    fn images_are_square_and_tinted() {
        let image = Sample::Dot.image(egui::Color32::RED, 18.0);
        assert_eq!(
            image.source(&egui::Context::default()).uri(),
            Some(Sample::Dot.uri())
        );
    }
}
