//! Captures this build's provenance for the About window: the manox-app commit
//! being built and the pinned GPUI stack versions resolved in the workspace
//! lockfile. Every value is injected as a `cargo:rustc-env` pair — empty when it
//! cannot be resolved — so a rerun never leaves a stale value behind and `pins`
//! reports "unknown" instead of a wrong string.

use std::{fs, path::Path, path::PathBuf, process::Command};

/// `[[package]]` entries as `(name, version, source)`, in lockfile order.
type Locked = Vec<(String, String, Option<String>)>;

fn main() {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.join("../..");
    let lock_path = repo_root.join("Cargo.lock");

    // The repo root is a worktree while a change is in flight, where `.git` is a
    // file pointing at the real git dir; watch that dir's HEAD and reflog so a
    // new commit re-runs this script.
    let git_dir = {
        let dot_git = repo_root.join(".git");
        if dot_git.is_file() {
            fs::read_to_string(&dot_git)
                .ok()
                .and_then(|s| s.strip_prefix("gitdir: ").map(|p| PathBuf::from(p.trim())))
                .unwrap_or(dot_git)
        } else {
            dot_git
        }
    };
    for path in [
        lock_path.clone(),
        git_dir.join("HEAD"),
        git_dir.join("logs").join("HEAD"),
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-changed=build.rs");

    let locked = fs::read_to_string(&lock_path)
        .ok()
        .and_then(|text| lock_packages(&text));
    let locked = locked.as_deref();
    let package = |name: &str| locked.and_then(|pkgs| pkgs.iter().find(|(n, _, _)| n == name));

    let gpui_component = package("gpui-component").map(|(_, version, _)| version.as_str());
    let gpui = package("gpui-pre").map(|(_, version, _)| version.as_str());
    let manox_rev = package("manox-agent")
        .and_then(|(_, _, source)| source.as_deref())
        .and_then(|source| source.strip_prefix("git+https://github.com/dspo/manox.git"))
        .and_then(|rest| rest.rsplit_once('#'))
        .map(|(_, rev)| rev);

    println!(
        "cargo:rustc-env=MANOX_APP_COMMIT={}",
        head_commit(&manifest_dir).unwrap_or_default()
    );
    println!(
        "cargo:rustc-env=GPUI_COMPONENT_VERSION={}",
        gpui_component.unwrap_or_default()
    );
    println!("cargo:rustc-env=GPUI_VERSION={}", gpui.unwrap_or_default());
    println!(
        "cargo:rustc-env=MANOX_UPSTREAM_REV={}",
        manox_rev.unwrap_or_default()
    );
}

/// Commit of the manox-app worktree this build reads from.
fn head_commit(manifest_dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(manifest_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        println!("cargo:warning=git rev-parse HEAD failed; About omits the manox-app commit");
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn lock_packages(lock: &str) -> Option<Locked> {
    let parsed: toml::Value = toml::from_str(lock).ok()?;
    Some(
        parsed
            .get("package")?
            .as_array()?
            .iter()
            .filter_map(|entry| {
                Some((
                    entry.get("name")?.as_str()?.to_string(),
                    entry.get("version")?.as_str()?.to_string(),
                    entry.get("source")?.as_str().map(str::to_string),
                ))
            })
            .collect(),
    )
}
