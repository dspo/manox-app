//! Sheet identity: `kind:instance`.
//!
//! A sheet id is the only thing a layout stores. It names one instance of one
//! kind — a start page, a session, a terminal resource — and it is stable
//! across restarts, so a parked or persisted layout can be read back without
//! asking a view anything.
//!
//! The three v1 constructors agree with the nsh-pi experiment's vocabulary
//! (`dock.ts`, `domains/pty.ts`, `modules/pi/sheets.ts`) on the wire:
//!
//! - `home:{container_id}` — the container's start page;
//! - `session:{session_id}` — a conversation; the id is container-neutral,
//!   because one session sheet may be shown in any container;
//! - `terminal:{container_id}:{n}` — the container's n-th terminal resource.
//!
//! `sheet_container` is the inverse only where the id encodes an owner: a
//! start page and a terminal do, a session does not (its container is the
//! workspace's answer, not the id's).

/// The kind portion of a sheet id (`kind:instance`).
pub fn sheet_kind(id: &str) -> &str {
    match id.find(':') {
        Some(at) => &id[..at],
        None => id,
    }
}

/// A container's start page: `home:{container_id}`.
pub fn home_sheet_id(container_id: &str) -> String {
    format!("home:{container_id}")
}

/// A session's sheet id: `session:{session_id}`.
pub fn session_sheet_id(session_id: &str) -> String {
    format!("session:{session_id}")
}

/// A container's n-th terminal resource: `terminal:{container_id}:{n}`.
pub fn terminal_sheet_id(container_id: &str, index: u32) -> String {
    format!("terminal:{container_id}:{index}")
}

/// The container a sheet id encodes, when it encodes one.
///
/// Start pages and terminals do; a session does not (see the module docs).
/// Kind-less or malformed ids answer `None` rather than guessing.
pub fn sheet_container(id: &str) -> Option<String> {
    match sheet_kind(id) {
        "home" => {
            let container = id.strip_prefix("home:")?;
            (!container.is_empty()).then(|| container.to_string())
        }
        "terminal" => terminal_resource(id).map(|resource| resource.container_id),
        _ => None,
    }
}

/// The session a sheet id names, when it names one.
pub fn session_of_sheet(id: &str) -> Option<&str> {
    let session = id.strip_prefix("session:")?;
    (!session.is_empty()).then_some(session)
}

/// A browser sheet id: `webview:{container_id}:{n}`.
pub fn webview_sheet_id(container_id: &str, index: u32) -> String {
    format!("webview:{container_id}:{index}")
}

/// An editor sheet id: `editor:{container_id}:{encoded_path}` (the path is
/// percent-encoded, one path segment like a container id).
pub fn editor_sheet_id(container_id: &str, path: &str) -> String {
    format!("editor:{container_id}:{}", encode_segment(path))
}

/// A diff sheet id: `diff:{container_id}:{old}~{new}`.
pub fn diff_sheet_id(container_id: &str, old: &str, new: &str) -> String {
    format!("diff:{container_id}:{old}~{new}")
}

/// A CLI agent sheet id: `agent:{container_id}:{n}` (the provider — claude /
/// codex / copilot — is the kind store's payload, not the id).
pub fn agent_sheet_id(container_id: &str, index: u32) -> String {
    format!("agent:{container_id}:{index}")
}

fn encode_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
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

/// Decodes one `%XX` escape starting at `index`; `Some((byte, 3))` on a
/// valid escape, `None` otherwise (the caller copies the byte verbatim).
fn percent_byte(bytes: &[u8], index: usize) -> Option<(u8, usize)> {
    if bytes.get(index) != Some(&b'%') {
        return None;
    }
    let high = hex_digit(*bytes.get(index + 1)?)?;
    let low = hex_digit(*bytes.get(index + 2)?)?;
    Some((high * 16 + low, 3))
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Decodes a percent-encoded segment back to bytes (the inverse of
/// `encode_segment` in `persist.rs`'s encoding face).
fn decode_segment(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match percent_byte(bytes, index) {
            Some((byte, width)) => {
                decoded.push(byte);
                index += width;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// A browser sheet's decoded resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebviewResource {
    pub container_id: String,
    pub index: u32,
}

/// Decode a browser sheet id.
pub fn webview_resource(id: &str) -> Option<WebviewResource> {
    let rest = id.strip_prefix("webview:")?;
    let (container, tail) = rest.rsplit_once(':')?;
    if container.is_empty() || !is_index(tail) {
        return None;
    }
    Some(WebviewResource {
        container_id: container.to_string(),
        index: tail.parse().unwrap_or(1),
    })
}

/// An editor sheet's decoded payload: the owning container and the file path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorResource {
    pub container_id: String,
    pub path: String,
}

/// Decode an editor sheet id.
pub fn editor_resource(id: &str) -> Option<EditorResource> {
    let rest = id.strip_prefix("editor:")?;
    let (container, encoded) = rest.split_once(':')?;
    if container.is_empty() || encoded.is_empty() {
        return None;
    }
    Some(EditorResource {
        container_id: container.to_string(),
        path: decode_segment(encoded),
    })
}

/// A diff sheet's decoded payload: the owning container and the pair of
/// versions the patch bridges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffResource {
    pub container_id: String,
    pub old: String,
    pub new: String,
}

/// Decode a diff sheet id.
pub fn diff_resource(id: &str) -> Option<DiffResource> {
    let rest = id.strip_prefix("diff:")?;
    let (container, pair) = rest.split_once(':')?;
    let (old, new) = pair.split_once('~')?;
    if container.is_empty() || old.is_empty() || new.is_empty() {
        return None;
    }
    Some(DiffResource {
        container_id: container.to_string(),
        old: old.to_string(),
        new: new.to_string(),
    })
}

/// A CLI agent sheet's decoded resource: the owning container and the
/// instance index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentResource {
    pub container_id: String,
    pub index: u32,
}

/// Decode a CLI agent sheet id.
pub fn agent_resource(id: &str) -> Option<AgentResource> {
    let rest = id.strip_prefix("agent:")?;
    let (container, tail) = rest.rsplit_once(':')?;
    if container.is_empty() || !is_index(tail) {
        return None;
    }
    Some(AgentResource {
        container_id: container.to_string(),
        index: tail.parse().unwrap_or(1),
    })
}

/// A terminal sheet's decoded resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalResource {
    /// The container that owns the resource.
    pub container_id: String,
    /// The container's resource index (1-based).
    pub index: u32,
}

/// Decode a terminal sheet id.
///
/// Container ids carry `:` themselves (`external:{agent}:{uuid}`), so the
/// index is the last `:`-segment and only when it is numeric. An id written
/// before terminals were numbered resources (`terminal:{container_id}`) is
/// resource 1 — the same tolerance the experiment's `terminalResource` keeps.
/// A numbered id whose index does not fit `u32` is malformed (`None`): folding
/// it onto resource 1 would collide with the sheet it is not.
pub fn terminal_resource(id: &str) -> Option<TerminalResource> {
    let rest = id.strip_prefix("terminal:")?;
    match rest.rsplit_once(':') {
        Some((container, tail)) if !container.is_empty() && is_index(tail) => {
            let index = tail.parse().ok()?;
            Some(TerminalResource {
                container_id: container.to_string(),
                index,
            })
        }
        _ if rest.is_empty() => None,
        _ => Some(TerminalResource {
            container_id: rest.to_string(),
            index: 1,
        }),
    }
}

fn is_index(tail: &str) -> bool {
    !tail.is_empty() && tail.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_the_prefix_before_the_first_colon() {
        assert_eq!(sheet_kind("home:abc"), "home");
        assert_eq!(sheet_kind("session:0f9e"), "session");
        assert_eq!(sheet_kind("terminal:external:pi:1234:2"), "terminal");
        assert_eq!(sheet_kind("bare"), "bare");
    }

    #[test]
    fn the_three_constructors_round_trip() {
        assert_eq!(home_sheet_id("c1"), "home:c1");
        assert_eq!(session_sheet_id("s1"), "session:s1");
        assert_eq!(terminal_sheet_id("c1", 2), "terminal:c1:2");

        assert_eq!(sheet_container("home:c1").as_deref(), Some("c1"));
        assert_eq!(session_of_sheet("session:s1"), Some("s1"));
        assert_eq!(
            terminal_resource("terminal:c1:2"),
            Some(TerminalResource {
                container_id: "c1".to_string(),
                index: 2
            })
        );
    }

    #[test]
    fn a_session_sheet_has_no_encoded_container() {
        assert_eq!(sheet_container("session:s1"), None);
    }

    #[test]
    fn terminal_ids_split_on_the_last_segment() {
        // An external container id carries colons of its own.
        let id = terminal_sheet_id("external:pi:1234", 3);
        assert_eq!(id, "terminal:external:pi:1234:3");
        assert_eq!(
            terminal_resource(&id),
            Some(TerminalResource {
                container_id: "external:pi:1234".to_string(),
                index: 3
            })
        );
        assert_eq!(sheet_container(&id).as_deref(), Some("external:pi:1234"));
    }

    #[test]
    fn an_unnumbered_terminal_is_resource_one() {
        assert_eq!(
            terminal_resource("terminal:c1"),
            Some(TerminalResource {
                container_id: "c1".to_string(),
                index: 1
            })
        );
    }

    #[test]
    fn an_out_of_range_terminal_index_is_malformed() {
        // An index past `u32` must not fold onto the resource it would
        // collide with (`terminal:c1:4294967296` vs `terminal:c1:1`).
        assert_eq!(terminal_resource("terminal:c1:4294967296"), None);
        assert_eq!(sheet_container("terminal:c1:4294967296"), None);
        assert_eq!(
            terminal_resource(&terminal_sheet_id("c1", u32::MAX)),
            Some(TerminalResource {
                container_id: "c1".to_string(),
                index: u32::MAX
            })
        );
    fn the_tool_sheet_ids_round_trip() {
        // Browser: the container keeps its colons; the index is the tail.
        let web = webview_sheet_id("external:pi:1234", 2);
        assert_eq!(web, "webview:external:pi:1234:2");
        assert_eq!(
            webview_resource(&web),
            Some(WebviewResource {
                container_id: "external:pi:1234".into(),
                index: 2
            })
        );

        // Editor: the path percent-encodes into one segment and decodes back.
        let editor = editor_sheet_id("c1", "/Users/me/a b.rs");
        assert_eq!(editor, "editor:c1:%2FUsers%2Fme%2Fa%20b.rs");
        assert_eq!(
            editor_resource(&editor),
            Some(EditorResource {
                container_id: "c1".into(),
                path: "/Users/me/a b.rs".into()
            })
        );

        // Diff: `old~new` splits on the tilde.
        let diff = diff_sheet_id("c1", "HEAD", "staged");
        assert_eq!(diff, "diff:c1:HEAD~staged");
        assert_eq!(
            diff_resource(&diff),
            Some(DiffResource {
                container_id: "c1".into(),
                old: "HEAD".into(),
                new: "staged".into()
            })
        );

        // CLI agent: the provider is the store's payload; the id stays two
        // segments.
        let agent = agent_sheet_id("external:pi:1234", 1);
        assert_eq!(agent, "agent:external:pi:1234:1");
        assert_eq!(
            agent_resource(&agent),
            Some(AgentResource {
                container_id: "external:pi:1234".into(),
                index: 1
            })
        );

        // The kinds are the first segment, as always.
        assert_eq!(sheet_kind(&web), "webview");
        assert_eq!(sheet_kind(&editor), "editor");
        assert_eq!(sheet_kind(&diff), "diff");
        assert_eq!(sheet_kind(&agent), "agent");
    }

    #[test]
    fn malformed_ids_answer_none() {
        assert_eq!(terminal_resource("terminal:"), None);
        assert_eq!(terminal_resource("home:c1"), None);
        assert_eq!(sheet_container("home:"), None);
        assert_eq!(session_of_sheet("session:"), None);
    }
}
