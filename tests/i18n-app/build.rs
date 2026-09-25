//! Compiles the fixture catalogs the way an app's build script does.

fn main() {
    fastframe_i18n::build::compile_catalogs("assets/i18n");
}
