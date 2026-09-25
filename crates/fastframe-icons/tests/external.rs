//! The macro as an app invokes it: from another crate, with the directory
//! relative to the invoking file.

fastframe_icons::icons! {
    /// Icons of a pretend app.
    #[derive(PartialOrd, Ord)]
    pub enum Icon {
        prefix: "app-icon-",
        directory: "fixtures/",
        /// A file of the app's own.
        Dot => "dot",
        /// A shared Lucide icon.
        Check => lucide "check",
    }
}

#[test]
fn an_app_declares_and_installs_its_icons() {
    assert_eq!(<Icon as fastframe_icons::IconSet>::all(), Icon::ALL);
    assert!(Icon::Dot < Icon::Check, "extra derives pass through");
    assert_eq!(Icon::Dot.bytes(), include_bytes!("fixtures/dot.svg"));
    assert_eq!(Icon::Check.bytes(), fastframe_icons::lucide::get("check"));

    let ctx = egui::Context::default();
    fastframe_icons::install::<Icon>(&ctx);
    for icon in Icon::ALL {
        assert!(matches!(
            ctx.try_load_bytes(icon.uri()),
            Ok(egui::load::BytesPoll::Ready { .. })
        ));
    }
}
