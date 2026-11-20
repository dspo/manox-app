//! The real `pi-acp` binary, handshake-level. Gated, because it needs a built
//! harness (`cd pi-harness && bun run build`) and, unlike the stub suite, no
//! model: this is the spawn/initialize/session-new/close smoke that proves the
//! shipped artifact speaks the protocol this crate implements.
//!
//! Run with:
//!
//! ```text
//! MANOX_RUN_LIVE=1 cargo test -p steer-acp --test live -- --ignored
//! ```
//!
//! `STEER_PI_ACP` overrides the binary; otherwise [`steer_acp::resolve_pi_acp`]
//! finds the repository build. The harness's own state (session files, settings)
//! is redirected to a temporary tree so the run leaves the user's `~/.pi` and
//! `~/.steer` untouched.

use std::time::Duration;

use steer_acp::{AcpEnv, AcpOptions, AcpProcess, mint_session_id, resolve_pi_acp};

#[tokio::test]
#[ignore = "needs a built pi-acp; enable with MANOX_RUN_LIVE=1"]
async fn handshake_and_offline_session_against_the_real_harness() {
    if std::env::var("MANOX_RUN_LIVE").as_deref() != Ok("1") {
        eprintln!("MANOX_RUN_LIVE is not 1; skipping the live pi-acp smoke");
        return;
    }
    let Some(bin) = resolve_pi_acp() else {
        panic!("no pi-acp found; run `cd pi-harness && bun run build` or set STEER_PI_ACP");
    };

    let home = std::env::temp_dir().join(format!("steer-acp-live-{}", std::process::id()));
    let agent_dir = home.join("pi-agent");
    std::fs::create_dir_all(&agent_dir).expect("the temporary state root is created");

    let process = AcpProcess::spawn(AcpOptions::new(
        bin.clone(),
        home.clone(),
        AcpEnv::for_state_dir(&home).with_agent_dir(&agent_dir),
    ))
    .unwrap_or_else(|error| panic!("cannot start {}: {error}", bin.display()));
    let _updates = process.updates().expect("updates are taken once");
    let _requests = process.requests().expect("requests are taken once");

    let initialized = tokio::time::timeout(
        Duration::from_secs(60),
        process.initialize("steer-acp", env!("CARGO_PKG_VERSION")),
    )
    .await
    .expect("initialize answers")
    .expect("initialize succeeds");
    assert_eq!(initialized.protocol_version, 1);
    assert_eq!(
        initialized.agent_name.as_deref(),
        Some("pi-harness"),
        "the shipped agent identifies itself"
    );

    let id = mint_session_id();
    let session = tokio::time::timeout(
        Duration::from_secs(60),
        process.new_session(Some(&id), &home),
    )
    .await
    .expect("session/new answers")
    .expect("session/new succeeds");
    assert_eq!(session.id(), id, "the host-minted id is the session id");
    assert!(
        session.info().config_options.is_array(),
        "session/new announces its config selectors"
    );

    tokio::time::timeout(Duration::from_secs(10), process.close())
        .await
        .expect("close settles");
    assert_eq!(
        process.exit().and_then(|exit| exit.code),
        Some(0),
        "stdin EOF is the graceful exit"
    );

    // Best effort: the harness may still be flushing its own files.
    let _ = std::fs::remove_dir_all(&home);
}
