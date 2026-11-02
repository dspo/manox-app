//! Build libghostty-vt and link it statically.
//!
//! Resolution order, per input:
//!
//! - **Ghostty sources**: `GHOSTTY_SRC` (a local checkout — the dev loop, the
//!   build is zig-incremental) or a pinned upstream tarball downloaded into
//!   `OUT_DIR` (the CI path; sha256-verified).
//! - **Zig**: `GHOSTTY_ZIG` → a `zig` on PATH with version ≥ 0.17 (Ghostty's
//!   own floor) → a pinned Zig toolchain downloaded into `OUT_DIR`.
//!
//! Force a rebuild with `cargo clean -p steer-ghostty`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Ghostty rev the FFI bindings in `src/sys.rs` were transcribed against.
/// Bump this together with the bindings (diff `include/ghostty/vt/*.h`).
const GHOSTTY_REV: &str = "042b16b6146ada29877dfeeea9f286889e4adeb1";
const GHOSTTY_TARBALL_SHA256: &str =
    "59055c41ac0be16a1e5bb2079312bac89181b94f82fd0399ca81682ea047b0c0";
/// Ghostty requires zig ≥ 0.17 (`build.zig.zon` `minimum_zig_version`).
const ZIG_VERSION: &str = "0.17.0";
const ZIG_MIN: (u32, u32) = (0, 17);

fn main() {
    println!("cargo:rerun-if-env-changed=GHOSTTY_SRC");
    println!("cargo:rerun-if-env-changed=GHOSTTY_ZIG");
    println!("cargo:rerun-if-changed=build.rs");

    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let lib = out.join("lib");
    if lib.join("libghostty-vt.a").exists() {
        link(&lib);
        return;
    }

    let src = resolve_sources(&out);
    let zig = resolve_zig(&out);
    build_libvt(&zig, &src, &out);
    link(&lib);
}

fn link(lib: &Path) {
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    if cfg!(target_os = "macos") {
        println!("cargo:rustc-link-lib=objc");
    }
}

fn build_libvt(zig: &Path, src: &Path, out: &Path) {
    let status = Command::new(zig)
        .args([
            "build",
            "-Demit-lib-vt",
            "-Doptimize=ReleaseFast",
            "--prefix",
        ])
        .arg(out)
        .current_dir(src)
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn zig ({}): {e}", zig.display()));
    assert!(status.success(), "zig build -Demit-lib-vt failed");
    assert!(
        out.join("lib/libghostty-vt.a").exists(),
        "zig build produced no libghostty-vt.a"
    );
}

// ---------------------------------------------------------------------------
// Ghostty sources
// ---------------------------------------------------------------------------

fn resolve_sources(out: &Path) -> PathBuf {
    if let Some(p) = std::env::var_os("GHOSTTY_SRC") {
        let src = PathBuf::from(p);
        assert!(
            src.join("build.zig").exists(),
            "GHOSTTY_SRC ({}) is not a Ghostty checkout (no build.zig)",
            src.display()
        );
        return src;
    }
    download_sources(out)
}

/// Download the pinned Ghostty tarball (codeload archive of `GHOSTTY_REV`)
/// and extract it under `OUT_DIR/ghostty-src`.
fn download_sources(out: &Path) -> PathBuf {
    let dir = out.join("ghostty-src");
    let mut found = None;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            if e.path().join("build.zig").exists() {
                found = Some(e.path());
                break;
            }
        }
    }
    if let Some(p) = found {
        return p;
    }

    let url = format!("https://codeload.github.com/ghostty-org/ghostty/tar.gz/{GHOSTTY_REV}");
    let tarball = out.join("ghostty-src.tar.gz");
    fetch(&url, &tarball);
    assert_sha256(&tarball, GHOSTTY_TARBALL_SHA256);
    std::fs::create_dir_all(&dir).unwrap();
    run(Command::new("tar")
        .arg("-xzf")
        .arg(&tarball)
        .arg("-C")
        .arg(&dir));
    std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.join("build.zig").exists())
        .expect("ghostty tarball extracted but no build.zig found")
}

// ---------------------------------------------------------------------------
// Zig toolchain
// ---------------------------------------------------------------------------

fn resolve_zig(out: &Path) -> PathBuf {
    if let Ok(p) = std::env::var("GHOSTTY_ZIG") {
        return PathBuf::from(p);
    }
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let candidate = dir.join("zig");
        if candidate.is_file()
            && let Some(v) = zig_version(&candidate)
            && v >= ZIG_MIN
        {
            return candidate;
        }
    }
    download_zig(out)
}

fn zig_version(zig: &Path) -> Option<(u32, u32)> {
    let out = Command::new(zig).arg("version").output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let mut parts = s.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Host zig tarball identifiers for the platforms this workspace CI builds on.
fn zig_tarball_id() -> (&'static str, &'static str) {
    match (
        std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
        std::env::var("CARGO_CFG_TARGET_ARCH").as_deref(),
    ) {
        (Ok("macos"), Ok("aarch64")) => (
            "aarch64-macos",
            "b607e9b9234790a008116ae5bdb71c6243b84b9fb42a53a9e70fde41c06c536a",
        ),
        (Ok("linux"), Ok("x86_64")) => (
            "x86_64-linux",
            "1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026",
        ),
        (os, arch) => panic!(
            "no pinned zig toolchain for os={os:?} arch={arch:?}; install zig >= 0.17 and set GHOSTTY_ZIG"
        ),
    }
}

fn download_zig(out: &Path) -> PathBuf {
    let (id, sha256) = zig_tarball_id();
    let dir = out.join("zig");
    let marker = dir.join(format!(".ready-{id}"));
    if marker.exists() {
        return zig_binary(&dir, id);
    }

    let url = format!("https://ziglang.org/download/{ZIG_VERSION}/zig-{id}-{ZIG_VERSION}.tar.xz");
    let tarball = out.join(format!("zig-{id}.tar.xz"));
    fetch(&url, &tarball);
    assert_sha256(&tarball, sha256);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    run(Command::new("tar")
        .arg("-xJf")
        .arg(&tarball)
        .arg("-C")
        .arg(&dir));
    std::fs::write(&marker, b"").unwrap();
    zig_binary(&dir, id)
}

fn zig_binary(dir: &Path, id: &str) -> PathBuf {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path().join("zig"))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("zig toolchain {id} extracted but no zig binary found"))
}

// ---------------------------------------------------------------------------
// fetch + verify helpers (curl/tar/sha256sum are present on both CI legs)
// ---------------------------------------------------------------------------

fn fetch(url: &str, dest: &Path) {
    run(Command::new("curl")
        .args(["-fSL", "--retry", "3", "-o"])
        .arg(dest)
        .arg(url));
}

fn run(cmd: &mut Command) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn {:?}: {e}", cmd));
    assert!(status.success(), "{cmd:?} failed");
}

fn assert_sha256(path: &Path, expected: &str) {
    let got = if which("sha256sum") {
        let out = Command::new("sha256sum")
            .arg(path)
            .output()
            .expect("sha256sum");
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .next()
            .unwrap()
            .to_string()
    } else {
        let out = Command::new("shasum")
            .args(["-a", "256"])
            .arg(path)
            .output()
            .expect("shasum");
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .next()
            .unwrap()
            .to_string()
    };
    assert_eq!(got, expected, "sha256 mismatch for {}", path.display());
}

fn which(prog: &str) -> bool {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .any(|dir| dir.join(prog).is_file())
}
