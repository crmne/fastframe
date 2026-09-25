//! The build-time PO compiler (feature `build`).
//!
//! Take the crate as a build-dependency with this feature and call
//! [`compile_catalogs`] from `build.rs`:
//!
//! ```no_run
//! // In build.rs, inside `main`:
//! fastframe_i18n::build::compile_catalogs("assets/i18n");
//! ```
//!
//! Each `assets/i18n/<tag>.po` becomes a module named after its tag in
//! lowercase with `-` turned into `_` (`pt-BR.po` is `pt_br`), holding a
//! `Translator` and its `PLURALS` count. `$OUT_DIR/catalogs.rs` declares them
//! all; include it where the app's `Locale` lives:
//!
//! ```ignore
//! include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));
//! ```
//!
//! Like `msgfmt`, the compiler leaves out fuzzy and unfinished entries, so an
//! untranslated message shows the English source. An incomplete plural
//! (any empty form) is left out whole: one empty form would otherwise hide
//! the label for some counts.
//!
//! Ordinary builds need no gettext tools; only updating the template does
//! (see `scripts/update-translations.sh` in this repository).

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use polib::message::{CatalogMessageMutView, MessageView};

/// A catalog that could not be compiled, with the file it came from.
#[derive(Debug)]
pub struct CatalogError {
    /// The PO file, or the directory when it could not be listed.
    pub path: PathBuf,
    /// What went wrong.
    pub source: Box<dyn Error + Send + Sync>,
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.source)
    }
}

impl Error for CatalogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// One compiled catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compiled {
    /// The module name the index declares: `pt_br` for `pt-BR.po`.
    pub module: String,
    /// The generated Rust file.
    pub path: PathBuf,
}

/// Compiles every `*.po` in `directory` into `$OUT_DIR` and writes the
/// module index `$OUT_DIR/catalogs.rs`, for `build.rs`.
///
/// Asks Cargo to rerun the build script when the directory changes, and
/// panics with the file name on any error, which fails the build as the
/// apps' build scripts did.
#[allow(
    clippy::print_stdout,
    reason = "Cargo reads build script instructions from standard output"
)]
pub fn compile_catalogs(directory: impl AsRef<Path>) -> Vec<Compiled> {
    let directory = directory.as_ref();
    println!("cargo:rerun-if-changed={}", directory.display());
    let output = PathBuf::from(
        std::env::var_os("OUT_DIR").unwrap_or_else(|| panic!("OUT_DIR is set by Cargo")),
    );
    compile_directory(directory, &output).unwrap_or_else(|error| panic!("{error}"))
}

/// Compiles every `*.po` in `directory` into `output` and writes the module
/// index `output/catalogs.rs`.
///
/// Modules are sorted by name so the index is the same on every machine.
pub fn compile_directory(directory: &Path, output: &Path) -> Result<Vec<Compiled>, CatalogError> {
    let failed = |path: &Path| {
        let path = path.to_path_buf();
        move |source: Box<dyn Error + Send + Sync>| CatalogError { path, source }
    };
    let entries = std::fs::read_dir(directory).map_err(|error| failed(directory)(error.into()))?;
    let mut compiled = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| failed(directory)(error.into()))?
            .path();
        if path.extension().is_none_or(|extension| extension != "po") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| failed(&path)("the file name is not UTF-8".into()))?;
        let module = module_name(stem)
            .ok_or_else(|| failed(&path)(format!("{stem:?} is not a catalog identifier").into()))?;
        let generated = output.join(stem).with_extension("rs");
        compile(&path, &generated).map_err(failed(&path))?;
        compiled.push(Compiled {
            module,
            path: generated,
        });
    }
    compiled.sort_by(|one, other| one.module.cmp(&other.module));
    let index: String = compiled
        .iter()
        .map(|catalog| format!("#[path = {:?}]\nmod {};\n", catalog.path, catalog.module))
        .collect();
    std::fs::write(output.join("catalogs.rs"), index)
        .map_err(|error| failed(&output.join("catalogs.rs"))(error.into()))?;
    Ok(compiled)
}

/// The module name for a catalog file stem: lowercase, `-` as `_`. `None`
/// when the result would not be a plain Rust identifier.
#[must_use]
pub fn module_name(stem: &str) -> Option<String> {
    let module = stem.replace('-', "_").to_ascii_lowercase();
    let valid = module
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && module
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    valid.then_some(module)
}

/// Compiles one PO file to a Rust module at `destination`.
///
/// Leaves out fuzzy and unfinished messages, then generates the lookup and
/// the plural rule. The prepared catalog is written next to `destination`
/// with a `.po` extension.
pub fn compile(source: &Path, destination: &Path) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut catalog = polib::po_file::parse(source).map_err(|error| error.to_string())?;
    let forms = catalog.metadata.plural_rules.nplurals;
    // Like msgfmt, do not ship fuzzy or unfinished translations. Checking every
    // plural form matters: an empty translation otherwise hides the whole label.
    for mut message in catalog.messages_mut() {
        let complete = match message.msgstr_plural() {
            Ok(translations) => {
                translations.len() == forms && translations.iter().all(|text| !text.is_empty())
            }
            Err(_) => message.msgstr().is_ok_and(|text| !text.is_empty()),
        };
        if message.is_fuzzy() || !complete {
            message.delete();
        }
    }
    let prepared = destination.with_extension("po");
    let mut writer = std::io::BufWriter::new(std::fs::File::create(&prepared)?);
    polib::po_file::write(&catalog, &mut writer).map_err(|error| error.to_string())?;
    std::io::Write::flush(&mut writer)?;
    include_po::generate_rs_from_po(&prepared, destination)?;
    let generated = std::fs::read_to_string(destination)?;
    std::fs::write(destination, adapt(&generated, forms)?)?;
    Ok(())
}

/// Adjusts include-po's output for the app that includes it.
///
/// The trait is named through this crate, so the app needs no direct `tr`
/// dependency. A constant plural rule (Japanese and Chinese: `plural=0`)
/// leaves `n` unused, and a catalog without plural messages leaves the
/// plural index unused; both are ignored explicitly rather than silencing
/// unused-variable warnings in the app. The items are `pub` inside the
/// app's private catalog modules, as include-po lays them out, so
/// `unreachable_pub` does not apply to them.
fn adapt(generated: &str, forms: usize) -> Result<String, String> {
    const TRAIT: &str = "::tr::Translator";
    const SIGNATURE: &str = "pub fn number_index(n: u64) -> u32 {";
    const INDEX: &str = "let ni = number_index(n);";
    const ATTRIBUTES: &str = "#![allow(dead_code)]";
    if [TRAIT, SIGNATURE, INDEX, ATTRIBUTES]
        .iter()
        .any(|expected| !generated.contains(expected))
    {
        return Err("the include-po output changed; update fastframe-i18n".into());
    }
    let mut adapted = generated
        .replace(TRAIT, "::fastframe_i18n::Translator")
        .replace(INDEX, &format!("{INDEX}\n        let _ = ni;"))
        .replace(ATTRIBUTES, "#![allow(dead_code, unreachable_pub)]");
    if forms == 1 {
        adapted = adapted.replace(SIGNATURE, &format!("{SIGNATURE}\n    let _ = n;"));
    }
    Ok(adapted)
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLISH: &str = r#"# Three plural forms.
msgid ""
msgstr ""
"Language: pl\n"
"Content-Type: text/plain; charset=UTF-8\n"
"Plural-Forms: nplurals=3; plural=(n == 1 ? 0 : n % 10 >= 2 && n % 10 <= 4 && (n % 100 < 12 || n % 100 > 14) ? 1 : 2);\n"

msgid "Home"
msgstr "Start"

msgid "Search"
msgstr ""

#, fuzzy
msgid "Library"
msgstr "Unreviewed translation"

msgid "Playlist • {count} song"
msgid_plural "Playlist • {count} songs"
msgstr[0] "Incomplete one"
msgstr[1] ""
msgstr[2] "Incomplete many"

msgid "{count} track"
msgid_plural "{count} tracks"
msgstr[0] "{count} utwór"
msgstr[1] "{count} utwory"
msgstr[2] "{count} utworów"
"#;

    const JAPANESE: &str = r#"msgid ""
msgstr ""
"Language: ja\n"
"Content-Type: text/plain; charset=UTF-8\n"
"Plural-Forms: nplurals=1; plural=0;\n"

msgid "{count} song"
msgid_plural "{count} songs"
msgstr[0] "{count}曲"
"#;

    #[test]
    fn unfinished_messages_are_left_out() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("pl.po");
        std::fs::write(&source, POLISH).unwrap();
        let destination = directory.path().join("out.rs");
        compile(&source, &destination).unwrap();
        let generated = std::fs::read_to_string(&destination).unwrap();
        assert!(generated.contains("\"Start\""));
        assert!(generated.contains("{count} utwory"));
        for left_out in [
            "Unreviewed translation",
            "Incomplete one",
            "Incomplete many",
        ] {
            assert!(!generated.contains(left_out), "{left_out}");
        }
        assert!(generated.contains("pub const PLURALS: usize = 3;"));
        assert!(generated.contains("impl ::fastframe_i18n::Translator for Translator"));
        assert!(!generated.contains("::tr::"));
    }

    #[test]
    fn a_constant_plural_rule_ignores_the_count_explicitly() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("ja.po");
        std::fs::write(&source, JAPANESE).unwrap();
        let destination = directory.path().join("ja.rs");
        compile(&source, &destination).unwrap();
        let generated = std::fs::read_to_string(&destination).unwrap();
        assert!(generated.contains("-> u32 {\n    let _ = n;"));
    }

    #[test]
    fn a_directory_compiles_to_a_sorted_module_index() {
        let directory = tempfile::tempdir().unwrap();
        let catalogs = directory.path().join("i18n");
        let output = directory.path().join("out");
        std::fs::create_dir_all(&catalogs).unwrap();
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(catalogs.join("pl.po"), POLISH).unwrap();
        std::fs::write(catalogs.join("pt-BR.po"), POLISH).unwrap();
        std::fs::write(catalogs.join("ja.po"), JAPANESE).unwrap();
        std::fs::write(catalogs.join("app.pot"), POLISH).unwrap();
        std::fs::write(catalogs.join("README"), "not a catalog").unwrap();
        let compiled = compile_directory(&catalogs, &output).unwrap();
        let modules: Vec<_> = compiled.iter().map(|c| c.module.as_str()).collect();
        assert_eq!(modules, ["ja", "pl", "pt_br"]);
        let index = std::fs::read_to_string(output.join("catalogs.rs")).unwrap();
        assert_eq!(index.matches("mod ").count(), 3);
        assert!(index.find("mod ja;").unwrap() < index.find("mod pt_br;").unwrap());
        assert!(output.join("pt-BR.rs").is_file());
    }

    #[test]
    fn a_broken_catalog_names_its_file() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("de.po"),
            "msgid \"x\"\nmsgstr \"y\"\n\"Plural-Forms: nonsense\n",
        )
        .unwrap();
        std::fs::write(directory.path().join("9x.po"), POLISH).unwrap();
        let error = compile_directory(directory.path(), directory.path()).unwrap_err();
        assert!(
            error.path.ends_with("de.po") || error.path.ends_with("9x.po"),
            "{error}"
        );
    }

    #[test]
    fn module_names_follow_the_tag() {
        assert_eq!(module_name("pt-BR").as_deref(), Some("pt_br"));
        assert_eq!(module_name("zh-Hant").as_deref(), Some("zh_hant"));
        assert_eq!(module_name("de").as_deref(), Some("de"));
        for bad in ["", "9x", "a.b", "a b", "-x"] {
            assert_eq!(module_name(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_changed_generator_is_reported_instead_of_miscompiled() {
        assert!(adapt("fn nothing() {}", 2).is_err());
    }
}
