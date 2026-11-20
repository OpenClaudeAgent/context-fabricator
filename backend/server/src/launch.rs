use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

#[cfg(all(target_os = "macos", debug_assertions))]
use super::credential_fixture;

const USAGE: &str = "usage: ctxfab-server DATA_DIRECTORY [--app-session]";

pub(super) struct LaunchOptions {
    pub(super) directory: PathBuf,
    pub(super) parent_pipe: bool,
    pub(super) github_fixture: Option<(u16, bool)>,
    pub(super) github_scheduled: bool,
    #[cfg(debug_assertions)]
    pub(super) sync_fixture: Option<bool>,
    #[cfg(all(target_os = "macos", debug_assertions))]
    pub(super) credential: Option<credential_fixture::CredentialFixture>,
}

pub(super) fn parse(
    mut args: impl Iterator<Item = OsString>,
) -> Result<LaunchOptions, Box<dyn Error>> {
    let directory = args.next().map(PathBuf::from).ok_or(USAGE)?;
    let mut next = args.next();
    let parent_pipe = next.as_deref() == Some(OsStr::new("--app-session"));
    if parent_pipe {
        next = args.next();
    }
    #[cfg(debug_assertions)]
    let sync_fixture = {
        let mode = match next.as_deref().and_then(OsStr::to_str) {
            Some("--sync-fixture") => Some(false),
            Some("--telemetry-fixture") => Some(true),
            _ => None,
        };
        if mode.is_some() {
            next = args.next();
            if next.is_some() {
                return Err(USAGE.into());
            }
        }
        mode
    };
    let github_fixture = None;
    let github_scheduled = !matches!(
        next.as_deref().and_then(OsStr::to_str),
        Some("--github-fixture" | "--github-keychain-fixture")
    );
    #[cfg(debug_assertions)]
    let github_fixture = match next.as_deref().and_then(OsStr::to_str) {
        Some(
            flag
            @ ("--github-fixture" | "--github-keychain-fixture" | "--github-scheduled-fixture"),
        ) => {
            let keychain = flag == "--github-keychain-fixture";
            let port = args
                .next()
                .and_then(|value| value.to_str()?.parse::<u16>().ok())
                .filter(|port| *port > 0)
                .ok_or(USAGE)?;
            next = args.next();
            Some((port, keychain))
        }
        _ => github_fixture,
    };
    #[cfg(all(target_os = "macos", debug_assertions))]
    let credential = next
        .take()
        .map(|flag| credential_fixture::CredentialFixture::parse(&flag, args.next().as_deref()))
        .transpose()?;
    if next.is_some() || args.next().is_some() {
        return Err(USAGE.into());
    }
    Ok(LaunchOptions {
        directory,
        parent_pipe,
        github_fixture,
        github_scheduled,
        #[cfg(debug_assertions)]
        sync_fixture,
        #[cfg(all(target_os = "macos", debug_assertions))]
        credential,
    })
}
