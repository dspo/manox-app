//! Sheet identity: `kind:instance`.
//!
//! A sheet id is the only thing a layout stores. It names one instance of one
//! kind — a start page, a session, a terminal resource — and it is stable
//! across restarts, so a parked or persisted layout can be read back without
//! asking a view anything.
//!
//! The constructors fix the id vocabulary — three kinds, one string shape
//! each, so a layout never needs a side table to name its sheets:
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
    }

    #[test]
    fn malformed_ids_answer_none() {
        assert_eq!(terminal_resource("terminal:"), None);
        assert_eq!(terminal_resource("home:c1"), None);
        assert_eq!(sheet_container("home:"), None);
        assert_eq!(session_of_sheet("session:"), None);
    }
}
