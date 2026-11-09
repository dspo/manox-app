//! Steer's in-app updater: a GitHub-Releases check → verified download →
//! in-place install pipeline behind one state machine.
//!
//! [`UpdateStore`] is the single state source: the About window and the app
//! menu project it, and every [`UpdateStore::set_state`] refreshes windows so
//! open surfaces re-render on each transition. The check and install seams
//! are macOS-only — the facade compiles everywhere, but on other platforms it
//! bails at those seams, because only macOS has a `.app` bundle the updater
//! can overwrite in place. Download and install orchestration runs detached
//! (`cx.spawn` + `background_spawn` with a blocking client — GPUI is not a
//! tokio runtime), so a job outlives the window or menu entry that started it.

pub mod artifact;
pub mod gh_release;
pub mod install;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use gpui::{App, AppContext as _, Global, SharedString};
use semver::Version;

pub use gh_release::ReleaseInfo;

/// Delay before the first automatic check: long enough to stay clear of the
/// first frame and the startup burst, short enough that a parked app learns
/// about a new release during the launch session.
const STARTUP_CHECK_DELAY: Duration = Duration::from_secs(8);

/// Automatic re-check cadence; far under the unauthenticated GitHub API
/// quota, and only ever at most one in-flight check behind the store guard.
const PERIODIC_CHECK_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);

/// One update lifecycle step. The store's `None` = idle: no check has found
/// anything, or the last one found the app up to date.
#[derive(Clone, Debug, PartialEq)]
pub enum UpdateState {
    /// A newer release exists; its artifact is neither downloaded nor verified.
    Available(ReleaseInfo),
    /// The release's dmg is streaming into the update cache.
    Downloading(ReleaseInfo),
    /// The artifact is verified and copied over the install position; the
    /// running process keeps its old image until it restarts.
    ReadyToInstall {
        release: ReleaseInfo,
        artifact: PathBuf,
    },
    /// The last action failed; `release` survives so surfaces can offer a
    /// retry of the same job.
    Failed {
        release: Option<ReleaseInfo>,
        message: SharedString,
    },
}

/// The app-wide update state behind a gpui global: one source the About
/// window, the menu, and the notification path all project.
pub struct UpdateStore(Option<UpdateState>);

impl Global for UpdateStore {}

impl UpdateStore {
    /// The current state, `None` before `init` or while idle.
    pub fn state(cx: &App) -> Option<UpdateState> {
        cx.try_global::<Self>().and_then(|store| store.0.clone())
    }

    /// Replace the state and re-render every open window, so all projecting
    /// surfaces (About window, menu) see the transition without per-surface
    /// subscriptions.
    pub fn set_state(cx: &mut App, state: Option<UpdateState>) {
        cx.set_global(Self(state));
        cx.refresh_windows();
    }

    pub fn set_available(cx: &mut App, release: Option<ReleaseInfo>) {
        Self::set_state(cx, release.map(UpdateState::Available));
    }

    pub fn set_downloading(cx: &mut App, release: ReleaseInfo) {
        Self::set_state(cx, Some(UpdateState::Downloading(release)));
    }

    pub fn set_failed(
        cx: &mut App,
        release: Option<ReleaseInfo>,
        message: impl Into<SharedString>,
    ) {
        Self::set_state(
            cx,
            Some(UpdateState::Failed {
                release,
                message: message.into(),
            }),
        );
    }

    pub fn is_downloading(cx: &App) -> bool {
        matches!(Self::state(cx), Some(UpdateState::Downloading(_)))
    }
}

/// The version this binary was built from; every workspace crate shares the
/// value, so reading it from this crate is the app's version.
pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is valid semver")
}

/// Query the repo's latest published release and fold it against `current`:
/// `Ok(None)` = not updatable (up to date, or a tag that refuses to parse),
/// `Err` = the check itself failed (network, unusable asset, missing digest).
pub fn check_latest(current: &Version) -> Result<Option<ReleaseInfo>> {
    let raw = gh_release::fetch_latest()?;
    gh_release::fold_release(&raw, current)
}

/// Install the store before any surface reads it (bin startup).
pub fn init(cx: &mut App) {
    cx.set_global(UpdateStore(None));
}

/// Fire one automatic check now: silent on failure (the store only ever
/// moves to `Available`, clears, or stays put), and any announcement of a
/// found release rides the bin's store observer rather than this crate.
pub fn check_now(cx: &mut App) {
    cx.spawn(async move |cx| run_scheduled_check(cx).await)
        .detach();
}

/// The automatic check schedule: one delayed first pass clear of the first
/// frame, then every 12h while the app lives. A pass only runs while the
/// store is idle or failed — never over an in-flight download/install. The
/// task is detached and outlives every surface that starts it.
pub fn spawn_startup_check(cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(STARTUP_CHECK_DELAY).await;
        run_scheduled_check(cx).await;
        loop {
            cx.background_executor()
                .timer(PERIODIC_CHECK_INTERVAL)
                .await;
            let due = cx.update(|cx| {
                matches!(
                    UpdateStore::state(cx),
                    None | Some(UpdateState::Failed { .. })
                )
            });
            if due {
                run_scheduled_check(cx).await;
            }
        }
    })
    .detach();
}

/// One scheduled pass against the live API.
async fn run_scheduled_check(cx: &gpui::AsyncApp) {
    let current = current_version();
    let result = cx
        .background_spawn(async move { check_latest(&current) })
        .await;
    cx.update(|cx| match result {
        Ok(Some(release)) => UpdateStore::set_available(cx, Some(release)),
        Ok(None) => UpdateStore::set_state(cx, None),
        Err(err) => tracing::warn!("automatic update check failed: {err:#}"),
    });
}

/// Start the download → verify → install job for the pending release. Also
/// the retry path: a `Failed` state that kept its release restarts the same
/// job. Detached — the job outlives the About window or menu entry that
/// triggered it.
pub fn start_update_download(cx: &mut App) {
    if UpdateStore::is_downloading(cx) {
        return;
    }
    let Some(release) = pending_release(cx) else {
        return;
    };
    // Fail before spending the download when there is nothing to overwrite.
    // The preflight has one failure mode per platform — no install position
    // (dev runs, non-macOS) — and that fixed refusal is chrome text:
    // localized, unlike the runtime error chains further down.
    if install::preflight().is_err() {
        UpdateStore::set_failed(cx, Some(release), steer_i18n::t("update-not-bundle"));
        return;
    }
    UpdateStore::set_downloading(cx, release.clone());
    cx.spawn(async move |cx| {
        let download = {
            let release = release.clone();
            cx.background_spawn(async move { artifact::download(&release) })
                .await
        };
        let outcome = match download {
            Ok(artifact) => {
                let installed = {
                    let artifact = artifact.clone();
                    cx.background_spawn(async move { install::install_artifact(&artifact) })
                        .await
                };
                installed.map(|()| artifact).map_err(|err| err.to_string())
            }
            Err(err) => Err(err.to_string()),
        };
        cx.update(|cx| match outcome {
            Ok(artifact) => UpdateStore::set_state(
                cx,
                Some(UpdateState::ReadyToInstall {
                    release: release.clone(),
                    artifact,
                }),
            ),
            Err(message) => UpdateStore::set_failed(cx, Some(release.clone()), message),
        });
    })
    .detach();
}

/// The release a download/install job can run for: an `Available` state, or
/// a `Failed` state that kept its release for the retry button.
fn pending_release(cx: &App) -> Option<ReleaseInfo> {
    match UpdateStore::state(cx) {
        Some(UpdateState::Available(release)) => Some(release),
        Some(UpdateState::Failed {
            release: Some(release),
            ..
        }) => Some(release),
        _ => None,
    }
}

/// The version whose availability has already been announced through a
/// notification. Recorded only after actual delivery, so a discovery made
/// while no window exists stays pending for the next window (re)open.
static NOTIFIED_VERSION: Mutex<Option<String>> = Mutex::new(None);

/// The `Available` release whose version has not yet been announced through
/// a notification, if any.
pub fn unnotified_release(cx: &App) -> Option<ReleaseInfo> {
    let notified = NOTIFIED_VERSION
        .lock()
        .expect("notified-version lock poisoned")
        .clone();
    match UpdateStore::state(cx)? {
        UpdateState::Available(release)
            if notified.as_deref() != Some(&release.version.to_string()) =>
        {
            Some(release)
        }
        _ => None,
    }
}

/// Record that `release`'s availability has been announced;
/// [`unnotified_release`] stops offering it. A still-newer version offered
/// later re-offers, so each release announces exactly once.
pub fn mark_notified(release: &ReleaseInfo) {
    *NOTIFIED_VERSION
        .lock()
        .expect("notified-version lock poisoned") = Some(release.version.to_string());
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    fn sample_release() -> ReleaseInfo {
        ReleaseInfo {
            version: "0.4.0".parse().expect("valid semver"),
            asset_name: "Steer_0.4.0_macos_arm64.dmg".into(),
            asset_url: "https://github.com/dspo/manox-app/releases/download/v0.4.0/Steer_0.4.0_macos_arm64.dmg".into(),
            asset_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
            asset_size: 1024,
            release_url: "https://github.com/dspo/manox-app/releases/tag/v0.4.0".into(),
        }
    }

    /// The full transition set the surfaces project: idle → available →
    /// downloading → ready / failed, and back to idle.
    #[gpui::test]
    fn update_store_transitions(cx: &mut TestAppContext) {
        cx.update(|cx| {
            init(cx);
            let release = sample_release();

            assert_eq!(UpdateStore::state(cx), None);

            UpdateStore::set_available(cx, Some(release.clone()));
            assert_eq!(
                UpdateStore::state(cx),
                Some(UpdateState::Available(release.clone()))
            );

            UpdateStore::set_state(cx, Some(UpdateState::Downloading(release.clone())));
            assert!(UpdateStore::is_downloading(cx));

            UpdateStore::set_state(
                cx,
                Some(UpdateState::ReadyToInstall {
                    release: release.clone(),
                    artifact: PathBuf::from("/tmp/Steer_0.4.0_macos_arm64.dmg"),
                }),
            );
            assert!(matches!(
                UpdateStore::state(cx),
                Some(UpdateState::ReadyToInstall { .. })
            ));

            UpdateStore::set_failed(cx, Some(release.clone()), "checksum mismatch");
            assert!(matches!(
                UpdateStore::state(cx),
                Some(UpdateState::Failed { .. })
            ));

            UpdateStore::set_state(cx, None);
            assert_eq!(UpdateStore::state(cx), None);
        });
    }

    /// A `Failed` state that kept its release is a valid starting point for
    /// the retry button; one that lost it (a failed check) is not.
    #[gpui::test]
    fn pending_release_survives_through_failed(cx: &mut TestAppContext) {
        cx.update(|cx| {
            init(cx);
            let release = sample_release();

            UpdateStore::set_failed(cx, Some(release.clone()), "rsync failed");
            assert_eq!(pending_release(cx), Some(release.clone()));

            UpdateStore::set_failed(cx, None, "GitHub Releases returned 403");
            assert_eq!(pending_release(cx), None);

            UpdateStore::set_available(cx, Some(release.clone()));
            assert_eq!(pending_release(cx), Some(release));

            UpdateStore::set_state(cx, None);
            assert_eq!(pending_release(cx), None);
        });
    }

    /// The announcement marker is per version: each release announces once,
    /// and only after `mark_notified` — discovery, delivery, marker in order.
    #[gpui::test]
    fn unnotified_release_tracks_the_announced_version(cx: &mut TestAppContext) {
        cx.update(|cx| {
            init(cx);
            let release = sample_release();

            UpdateStore::set_available(cx, Some(release.clone()));
            assert_eq!(unnotified_release(cx), Some(release.clone()));
            mark_notified(&release);
            assert_eq!(unnotified_release(cx), None);

            // A newer candidate re-offers until it is announced in turn.
            let mut newer = release.clone();
            newer.version = "0.5.0".parse().expect("valid semver");
            UpdateStore::set_available(cx, Some(newer.clone()));
            assert_eq!(unnotified_release(cx), Some(newer.clone()));
            mark_notified(&newer);
            assert_eq!(unnotified_release(cx), None);
        });
    }

    /// Live, read-only pass against the real Releases API; gated like the
    /// rest of the repo's live tests, so default runs stay offline.
    #[test]
    fn live_latest_release_folds() {
        if std::env::var_os("MANOX_RUN_LIVE").is_none() {
            return;
        }
        let folded = check_latest(&current_version()).expect("live check");
        if let Some(release) = folded {
            assert!(release.asset_name.ends_with("_macos_arm64.dmg"));
            assert_eq!(release.asset_sha256.len(), 64);
            assert!(release.asset_size > 0);
        }
    }
}
