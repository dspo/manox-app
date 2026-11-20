//! Layout persistence: one file per container, under `<steer_home>/layouts/`.
//!
//! The file is the canvas as it was left — `{containerId, layout, zoom,
//! updatedAt}`. It is written by whichever layer owns the save pump (the
//! canvas UI debounces 400ms and calls here); this module is synchronous and
//! atomic, and never keeps state of its own.
//!
//! A corrupt or unreadable file is warned about, dropped, and read as "no
//! file": the caller then opens the container's default canvas. There is no
//! repair path and no compatibility read — a layout format that no longer
//! parses is not worth carrying forward.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::layout::DockLayout;
use crate::state::CanvasSnapshot;

/// The layouts subtree of the Steer state root (app layer, harness-neutral).
const LAYOUTS_DIR: &str = "layouts";

/// One container's canvas as persisted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasFile {
    /// The container the file belongs to (also encoded into the file name).
    pub container_id: String,
    pub layout: DockLayout,
    /// The zoomed sheet, if the canvas was left zoomed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<String>,
    /// Write time, milliseconds since the epoch (diagnostics only — the file
    /// name, not the stamp, decides whose layout this is).
    pub updated_at: i64,
}

impl CanvasFile {
    /// A file stamped with the current time.
    pub fn new(container_id: impl Into<String>, layout: DockLayout, zoom: Option<String>) -> Self {
        Self {
            container_id: container_id.into(),
            layout,
            zoom,
            updated_at: now_millis(),
        }
    }

    /// The canvas as restore consumes it.
    pub fn snapshot(&self) -> CanvasSnapshot {
        CanvasSnapshot {
            layout: self.layout.clone(),
            zoom: self.zoom.clone(),
        }
    }
}

/// `<steer_home>/layouts`, or `None` when no home resolves.
pub fn layouts_dir() -> Option<PathBuf> {
    steer_paths::steer_home().map(|home| home.join(LAYOUTS_DIR))
}

/// The file a container's canvas is stored in, encoded to one path segment.
pub fn layout_path(container_id: &str) -> Option<PathBuf> {
    layouts_dir().map(|dir| dir.join(file_name(container_id)))
}

/// Reads a container's canvas from the real state root. `None` covers both
/// "no file yet" and "unreadable file" (the latter is warned about).
pub fn read_canvas(container_id: &str) -> Option<CanvasFile> {
    read_from(&layouts_dir()?, container_id)
}

/// Writes a canvas to the real state root, creating the layouts directory.
pub fn write_canvas(file: &CanvasFile) -> io::Result<()> {
    let dir = layouts_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no home directory; nowhere to root ~/.steer",
        )
    })?;
    write_to(&dir, file)
}

/// Reads one container's canvas from `dir`.
pub fn read_from(dir: &Path, container_id: &str) -> Option<CanvasFile> {
    let path = dir.join(file_name(container_id));
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "canvas layout is unreadable; opening the default canvas"
            );
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(file) => Some(file),
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "canvas layout does not parse; dropping it and opening the default canvas"
            );
            None
        }
    }
}

/// Writes one container's canvas into `dir` (created when missing), through a
/// temporary file so a reader never sees half a layout.
pub fn write_to(dir: &Path, file: &CanvasFile) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join(file_name(&file.container_id));
    let temp = dir.join(format!("{}.tmp", file_name(&file.container_id)));
    let body = serde_json::to_vec_pretty(file).map_err(io::Error::other)?;
    fs::write(&temp, body)?;
    fs::rename(&temp, &path)
}

/// A container id as one file name: everything outside the unreserved URI set
/// is percent-encoded (container ids carry `:` and may carry anything else).
pub fn encode_id(container_id: &str) -> String {
    let mut encoded = String::with_capacity(container_id.len());
    for byte in container_id.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                encoded.push('%');
                encoded.push(HEX[(byte >> 4) as usize] as char);
                encoded.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    encoded
}

fn file_name(container_id: &str) -> String {
    format!("{}.json", encode_id(container_id))
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{home_sheet_id, session_sheet_id};
    use crate::layout;

    fn temp_root(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new().prefix(name).tempdir().unwrap()
    }

    fn canvas(container_id: &str) -> CanvasFile {
        let layout = layout::append_sheet(
            &DockLayout::for_sheets([home_sheet_id(container_id)]),
            &session_sheet_id("s1"),
            None,
        );
        CanvasFile::new(container_id, layout, Some(session_sheet_id("s1")))
    }

    #[test]
    fn a_canvas_round_trips_through_its_file() {
        let root = temp_root("steer-canvas-persist");
        let dir = root.path().join(LAYOUTS_DIR);
        let file = canvas("external:pi:1234");
        write_to(&dir, &file).unwrap();

        let back = read_from(&dir, "external:pi:1234").expect("the file reads back");
        assert_eq!(back, file);
        assert_eq!(
            layout::panel_ids(&back.layout),
            ["home:external:pi:1234", "session:s1"]
        );
        assert_eq!(
            back.snapshot(),
            CanvasSnapshot {
                layout: file.layout.clone(),
                zoom: file.zoom.clone()
            }
        );
    }

    #[test]
    fn no_file_means_no_canvas() {
        let root = temp_root("steer-canvas-missing");
        assert_eq!(read_from(root.path(), "c1"), None);
    }

    #[test]
    fn a_corrupt_file_is_dropped_rather_than_half_read() {
        let root = temp_root("steer-canvas-corrupt");
        std::fs::write(root.path().join(file_name("c1")), "{ not json").unwrap();
        assert_eq!(read_from(root.path(), "c1"), None);
        // The bad file is left in place: the next write replaces it.
        assert!(root.path().join(file_name("c1")).is_file());
    }

    #[test]
    fn a_write_replaces_the_previous_canvas_atomically() {
        let root = temp_root("steer-canvas-rewrite");
        let dir = root.path().join(LAYOUTS_DIR);
        write_to(&dir, &canvas("c1")).unwrap();
        let empty = CanvasFile::new("c1", DockLayout::default(), None);
        write_to(&dir, &empty).unwrap();

        assert_eq!(read_from(&dir, "c1").unwrap().layout, DockLayout::default());
        // The temporary file never survives a completed write.
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "only the layout file itself remains"
        );
    }

    #[test]
    fn the_file_shape_is_the_persisted_contract() {
        let root = temp_root("steer-canvas-shape");
        let dir = root.path().join(LAYOUTS_DIR);
        let file = canvas("external:pi:1234");
        write_to(&dir, &file).unwrap();

        let text = std::fs::read_to_string(dir.join("external%3Api%3A1234.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["containerId"], "external:pi:1234");
        assert_eq!(value["zoom"], "session:s1");
        assert_eq!(value["layout"]["center"]["type"], "tabs");
        assert_eq!(
            value["layout"]["center"]["panels"][0],
            "home:external:pi:1234"
        );
        assert!(value["updatedAt"].is_i64());
    }

    #[test]
    fn every_container_id_encodes_to_one_path_segment() {
        for id in [
            "c1",
            "external:pi:1234",
            "session/with/slashes",
            "..",
            "a b",
        ] {
            let name = file_name(id);
            assert!(!name.contains('/'), "{id} encodes to {name}");
            assert!(name.ends_with(".json"), "{id} encodes to {name}");
            assert_eq!(
                Path::new(&name).components().count(),
                1,
                "{id} encodes to {name}"
            );
        }
        // The unreserved set passes through verbatim.
        assert_eq!(encode_id("0f9e-1234_ab.c~d"), "0f9e-1234_ab.c~d");
        assert_eq!(encode_id("external:pi:1"), "external%3Api%3A1");
    }
}
