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
    /// The verified artifact has been copied over the install position; the
    /// running process keeps its old image until it restarts.
    ReadyToInstall(ReleaseInfo),
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

/// Whether a check may run right now: only while no job is in flight. A
/// check that landed `Available` over a `Downloading`/`ReadyToInstall` state
/// would re-arm the download button and start a second job racing the first
/// on the same `.part` file — both this entry gate and
/// [`apply_check_success`] refuse that transition.
pub fn check_allowed(cx: &App) -> bool {
    !matches!(
        UpdateStore::state(cx),
        Some(UpdateState::Downloading(_)) | Some(UpdateState::ReadyToInstall(_))
    )
}

/// Store half of a completed check, behind the same gate as the entry: the
/// request outlives the state it started in (the user can start a download
/// while a check is still in flight), so a result that would land on
/// `Downloading`/`ReadyToInstall` is dropped instead of overwriting the job.
/// Returns whether the result reached the store. An `Err` result is the
/// caller's to surface — the manual path shows it in the About row, the
/// scheduled path only warns.
pub fn apply_check_success(cx: &mut App, found: Option<ReleaseInfo>) -> bool {
    if !check_allowed(cx) {
        return false;
    }
    match found {
        Some(release) => UpdateStore::set_available(cx, Some(release)),
        None => UpdateStore::set_state(cx, None),
    }
    true
}

/// The automatic check's gate, on top of [`check_allowed`]: a scheduled pass
/// runs only when the store is idle or failed (a known `Available` release
/// keeps its announcement rather than being re-discovered every 12h), and
/// only where the updater could actually install — a dev build (no bundle)
/// stays silent instead of alarming on every `cargo run`.
fn scheduled_check_due(cx: &App) -> bool {
    store_checkable(&UpdateStore::state(cx)) && install::preflight().is_ok()
}

/// The store half of the scheduled gate; the preflight is the other half.
fn store_checkable(state: &Option<UpdateState>) -> bool {
    matches!(state, None | Some(UpdateState::Failed { .. }))
}

/// The automatic check schedule: one delayed first pass clear of the first
/// frame, then every 12h while the app lives. Every pass goes through
/// [`scheduled_check_due`], so a pass never runs over an in-flight
/// download/install and never fires in a dev build. The task is detached and
/// outlives every surface that starts it.
pub fn spawn_startup_check(cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(STARTUP_CHECK_DELAY).await;
        run_scheduled_check(cx).await;
        loop {
            cx.background_executor()
                .timer(PERIODIC_CHECK_INTERVAL)
                .await;
            run_scheduled_check(cx).await;
        }
    })
    .detach();
}

/// One scheduled pass against the live API: silent on failure and on a
/// skipped gate (the store only ever moves to `Available`, clears, or stays
/// put), and any announcement of a found release rides the bin's store
/// observer rather than this crate.
async fn run_scheduled_check(cx: &gpui::AsyncApp) {
    if !cx.update(|cx| scheduled_check_due(cx)) {
        return;
    }
    let current = current_version();
    let result = cx
        .background_spawn(async move { check_latest(&current) })
        .await;
    cx.update(|cx| {
        // The request may have outlived the state it started in; re-check
        // before writing so a download that began mid-check keeps its state.
        if !scheduled_check_due(cx) {
            return;
        }
        match result {
            Ok(found) => {
                apply_check_success(cx, found);
            }
            Err(err) => tracing::warn!("automatic update check failed: {err:#}"),
        }
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
            // The artifact's content now lives in the install position and
            // nothing consumes the dmg again (restart re-execs the bundle),
            // so it leaves the cache instead of accruing ~80MB per update.
            Ok(artifact) => {
                let _ = std::fs::remove_file(&artifact);
                UpdateStore::set_state(cx, Some(UpdateState::ReadyToInstall(release.clone())));
            }
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

/// Read the notified-version marker, recovering the guard if a task panicked
/// while holding it — a poisoned marker must never take the app down.
fn notified_version() -> Option<String> {
    let guard = NOTIFIED_VERSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.clone()
}

/// The `Available` release whose version has not yet been announced through
/// a notification, if any.
pub fn unnotified_release(cx: &App) -> Option<ReleaseInfo> {
    let notified = notified_version();
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
    let mut guard = NOTIFIED_VERSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(release.version.to_string());
}

/// How long the inline "up to date" note stays visible in the About window
/// after a manual check that found nothing.
pub const UP_TO_DATE_VISIBLE: Duration = Duration::from_secs(8);

/// When the last "up to date" check happened, if any. Unlike the store (whose
/// idle state is indistinguishable between "never checked" and "checked, found
/// nothing"), this is the manual check's ephemeral answer: it shows for
/// [`UP_TO_DATE_VISIBLE`] and then expires.
static UP_TO_DATE_AT: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// Whether a mark made at `mark` is still inside its visibility window at
/// `now`.
fn up_to_date_visible_since(mark: std::time::Instant, now: std::time::Instant) -> bool {
    now.duration_since(mark) < UP_TO_DATE_VISIBLE
}

/// Record that a check just found the app up to date, and schedule the
/// refresh that retires the note when its visibility window ends (a newer
/// mark keeps the note alive — the clear only fires when the window has
/// actually elapsed).
pub fn mark_up_to_date(cx: &mut App) {
    *UP_TO_DATE_AT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(std::time::Instant::now());
    cx.refresh_windows();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(UP_TO_DATE_VISIBLE).await;
        cx.update(|cx| {
            let expired = UP_TO_DATE_AT
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .map(|mark| !up_to_date_visible_since(mark, std::time::Instant::now()))
                .unwrap_or(true);
            if expired {
                *UP_TO_DATE_AT
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
                cx.refresh_windows();
            }
        });
    })
    .detach();
}

/// Whether the About window should show the "up to date" note right now.
pub fn up_to_date_visible() -> bool {
    UP_TO_DATE_AT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some_and(|mark| up_to_date_visible_since(mark, std::time::Instant::now()))
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

            UpdateStore::set_state(cx, Some(UpdateState::ReadyToInstall(release.clone())));
            assert!(matches!(
                UpdateStore::state(cx),
                Some(UpdateState::ReadyToInstall(_))
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

    /// A check may never run over an in-flight job: landing `Available` on a
    /// `Downloading`/`ReadyToInstall` state would re-arm the download button
    /// and start a second job racing the first on the same `.part` file.
    #[gpui::test]
    fn checks_are_refused_while_a_job_is_in_flight(cx: &mut TestAppContext) {
        cx.update(|cx| {
            init(cx);
            let release = sample_release();

            assert!(check_allowed(cx));
            UpdateStore::set_available(cx, Some(release.clone()));
            assert!(check_allowed(cx));
            UpdateStore::set_failed(cx, Some(release.clone()), "rsync failed");
            assert!(check_allowed(cx));

            UpdateStore::set_state(cx, Some(UpdateState::Downloading(release.clone())));
            assert!(!check_allowed(cx));
            assert_eq!(pending_release(cx), None);

            UpdateStore::set_state(cx, Some(UpdateState::ReadyToInstall(release.clone())));
            assert!(!check_allowed(cx));
            assert_eq!(pending_release(cx), None);
        });
    }

    /// A completed check must not write over a job that started while the
    /// request was in flight: the entry gate alone cannot see that transition.
    #[gpui::test]
    fn a_completed_check_never_writes_over_an_in_flight_job(cx: &mut TestAppContext) {
        cx.update(|cx| {
            init(cx);
            let release = sample_release();

            UpdateStore::set_state(cx, Some(UpdateState::Downloading(release.clone())));
            assert!(!apply_check_success(cx, Some(release.clone())));
            assert!(!apply_check_success(cx, None));
            assert!(matches!(
                UpdateStore::state(cx),
                Some(UpdateState::Downloading(_))
            ));

            UpdateStore::set_state(cx, Some(UpdateState::ReadyToInstall(release.clone())));
            assert!(!apply_check_success(cx, Some(release.clone())));
            assert!(!apply_check_success(cx, None));
            assert!(matches!(
                UpdateStore::state(cx),
                Some(UpdateState::ReadyToInstall(_))
            ));

            // Idle and failed stores still take the result.
            UpdateStore::set_state(cx, None);
            assert!(apply_check_success(cx, Some(release.clone())));
            assert_eq!(
                UpdateStore::state(cx),
                Some(UpdateState::Available(release))
            );
            assert!(apply_check_success(cx, None));
            assert_eq!(UpdateStore::state(cx), None);
        });
    }

    /// The scheduled gate's store half: idle and failed states re-check,
    /// everything else (a known `Available`, an in-flight job) does not.
    #[test]
    fn the_scheduled_gate_re_checks_only_idle_and_failed_states() {
        let release = sample_release();
        assert!(store_checkable(&None));
        assert!(store_checkable(&Some(UpdateState::Failed {
            release: Some(release.clone()),
            message: "x".into(),
        })));
        assert!(!store_checkable(&Some(UpdateState::Available(
            release.clone()
        ))));
        assert!(!store_checkable(&Some(UpdateState::Downloading(
            release.clone()
        ))));
        assert!(!store_checkable(&Some(UpdateState::ReadyToInstall(
            release.clone()
        ))));
    }

    /// The "up to date" note lives exactly for its visibility window.
    #[test]
    fn the_up_to_date_note_expires_with_its_window() {
        let now = std::time::Instant::now();
        assert!(up_to_date_visible_since(now, now));
        assert!(up_to_date_visible_since(
            now,
            now + UP_TO_DATE_VISIBLE - Duration::from_secs(1)
        ));
        assert!(!up_to_date_visible_since(now, now + UP_TO_DATE_VISIBLE));
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
