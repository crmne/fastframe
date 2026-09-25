//! The first thing `main` does: run the helper when asked to, and take the
//! update flags off the command line.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use crate::host::{Host, OsHost};
use crate::{APPLY_UPDATE_FLAG, UPDATE_ERROR_FLAG, UPDATE_RECEIPT_FLAG, UpdateConfig};

/// A normal launch, with the update flags taken out.
#[derive(Debug)]
pub struct Launch {
    /// The command line without `--update-receipt` and `--update-error`,
    /// starting with the program name, for the app's own parser.
    pub arguments: Vec<OsString>,
    /// Set when the helper relaunched this app after installing it.
    /// Acknowledge it once the window is up; until then, the helper stands
    /// ready to roll back. Apps also use it to show the window even when
    /// they would start hidden.
    pub receipt: Option<Receipt>,
    /// Set when the helper restored the previous version: a message for the
    /// user. The helper sends English; an app that translates can match
    /// [`Launch::RESTORED`].
    pub error: Option<String>,
}

impl Launch {
    /// The message the helper passes to a restored app.
    pub const RESTORED: &'static str = crate::helper::RESTORED;
}

/// Proof that the helper relaunched this app after an update.
pub struct Receipt {
    job: PathBuf,
    config: UpdateConfig,
    host: Arc<dyn Host>,
}

impl std::fmt::Debug for Receipt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Receipt")
            .field("job", &self.job)
            .finish_non_exhaustive()
    }
}

impl Receipt {
    /// The `handoff.json` the helper named.
    pub fn path(&self) -> &Path {
        &self.job
    }

    /// Tells the helper the update started, so it keeps it. Call it after
    /// the first frame is drawn, off the interface thread. Fails, and lets
    /// the helper roll back, when the receipt is for another installation or
    /// version.
    pub fn acknowledge(&self) -> Result<()> {
        crate::helper::acknowledge(&self.config, self.host.as_ref(), &self.job)
    }
}

/// Handles this process's command line for the updater.
///
/// With `<app> --apply-update <job>` this process is the helper: it installs
/// the update and exits, printing any error to standard error (which the
/// handoff sends to `helper.log`). The exit code is 0 on success and 1 on
/// failure. Otherwise it returns the [`Launch`].
///
/// Call it first in `main`, before argument parsing and before any other
/// state (single-instance locks, logging to shared files) is set up.
pub fn intercept(config: &UpdateConfig) -> Launch {
    match parse(config, Arc::new(OsHost), std::env::args_os().collect()) {
        Parsed::Helper(job) => {
            let result = crate::helper::run(config, &OsHost, &job);
            if let Err(error) = &result {
                #[allow(
                    clippy::print_stderr,
                    reason = "the helper has no other output; its standard error is helper.log"
                )]
                {
                    eprintln!("{error:#}");
                }
            }
            std::process::exit(if result.is_ok() { 0 } else { 1 })
        }
        Parsed::Launch(launch) => *launch,
    }
}

#[derive(Debug)]
pub(crate) enum Parsed {
    Helper(PathBuf),
    Launch(Box<Launch>),
}

/// The helper runs only for exactly `<program> --apply-update <job>`, as
/// every earlier version starts it. The other flags may appear anywhere,
/// as `--flag value` (how helpers pass them) or `--flag=value`.
pub(crate) fn parse(
    config: &UpdateConfig,
    host: Arc<dyn Host>,
    arguments: Vec<OsString>,
) -> Parsed {
    if arguments.len() == 3 && arguments[1] == APPLY_UPDATE_FLAG {
        return Parsed::Helper(PathBuf::from(&arguments[2]));
    }
    let mut kept = Vec::with_capacity(arguments.len());
    let mut receipt = None;
    let mut error = None;
    let mut arguments = arguments.into_iter().peekable();
    if let Some(program) = arguments.next() {
        kept.push(program);
    }
    while let Some(argument) = arguments.next() {
        if argument == "--" {
            kept.push(argument);
            kept.extend(arguments.by_ref());
            break;
        }
        let text = argument.to_string_lossy();
        let (flag, inline) = match text.split_once('=') {
            Some((flag, value)) => (flag.to_owned(), Some(OsString::from(value))),
            None => (text.into_owned(), None),
        };
        if flag != UPDATE_RECEIPT_FLAG && flag != UPDATE_ERROR_FLAG {
            kept.push(argument);
            continue;
        }
        let Some(value) = inline.or_else(|| arguments.next()) else {
            // A flag without a value is left for the app's parser to reject.
            kept.push(argument);
            continue;
        };
        if flag == UPDATE_RECEIPT_FLAG {
            receipt = Some(Receipt {
                job: PathBuf::from(value),
                config: *config,
                host: Arc::clone(&host),
            });
        } else {
            error = Some(value.to_string_lossy().into_owned());
        }
    }
    Parsed::Launch(Box::new(Launch {
        arguments: kept,
        receipt,
        error,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeHost;
    use crate::tests::ZAPFAST;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn launch(values: &[&str]) -> Launch {
        match parse(&ZAPFAST, Arc::new(FakeHost::default()), args(values)) {
            Parsed::Launch(launch) => *launch,
            Parsed::Helper(job) => panic!("unexpected helper for {job:?}"),
        }
    }

    #[test]
    fn the_helper_runs_only_for_the_exact_command_older_versions_use() {
        match parse(
            &ZAPFAST,
            Arc::new(FakeHost::default()),
            args(&[
                "zapfast",
                "--apply-update",
                "/apps/.zapfast-update-0123456789abcdef/handoff.json",
            ]),
        ) {
            Parsed::Helper(job) => assert_eq!(
                job,
                Path::new("/apps/.zapfast-update-0123456789abcdef/handoff.json")
            ),
            other => panic!("{other:?}"),
        }
        for values in [
            &["zapfast", "--apply-update"][..],
            &["zapfast", "--verbose", "--apply-update", "job"],
            &["zapfast", "--apply-update", "job", "--verbose"],
        ] {
            let launch = launch(values);
            assert_eq!(launch.arguments, args(values));
        }
    }

    #[test]
    fn a_relaunch_from_the_helper_yields_a_receipt_and_clean_arguments() {
        // How every helper so far relaunches: the app's own arguments, then
        // the flag and the job.
        let launch = launch(&[
            "/apps/zapfast",
            "--verbose",
            "--update-receipt",
            "/apps/.zapfast-update-0123456789abcdef/handoff.json",
        ]);
        assert_eq!(launch.arguments, args(&["/apps/zapfast", "--verbose"]));
        assert_eq!(
            launch.receipt.unwrap().path(),
            Path::new("/apps/.zapfast-update-0123456789abcdef/handoff.json")
        );
        assert_eq!(launch.error, None);
    }

    #[test]
    fn a_restart_after_rollback_yields_the_message() {
        let launch = launch(&[
            "zapfast",
            "--start-hidden",
            "--update-error",
            "The update could not start. The previous version has been restored.",
        ]);
        assert_eq!(launch.arguments, args(&["zapfast", "--start-hidden"]));
        assert_eq!(launch.error.as_deref(), Some(Launch::RESTORED));
        assert!(launch.receipt.is_none());
    }

    #[test]
    fn equals_forms_and_missing_values_are_handled_like_clap() {
        let launch = launch(&[
            "zapfast",
            "--update-error=boom",
            "--update-receipt=/x/handoff.json",
        ]);
        assert_eq!(launch.arguments, args(&["zapfast"]));
        assert_eq!(launch.error.as_deref(), Some("boom"));
        assert_eq!(launch.receipt.unwrap().path(), Path::new("/x/handoff.json"));
        let launch = self::launch(&["zapfast", "--update-receipt"]);
        assert_eq!(launch.arguments, args(&["zapfast", "--update-receipt"]));
        assert!(launch.receipt.is_none());
    }

    #[test]
    fn arguments_after_a_double_dash_are_left_alone() {
        let launch = launch(&["zapfast", "--", "--update-error", "x"]);
        assert_eq!(
            launch.arguments,
            args(&["zapfast", "--", "--update-error", "x"])
        );
        assert!(launch.error.is_none());
    }

    #[test]
    fn a_plain_launch_is_unchanged() {
        let launch = launch(&["zapfast", "--verbose", "reload-themes"]);
        assert_eq!(
            launch.arguments,
            args(&["zapfast", "--verbose", "reload-themes"])
        );
        assert!(launch.receipt.is_none() && launch.error.is_none());
        assert!(self::launch(&[]).arguments.is_empty());
    }
}
