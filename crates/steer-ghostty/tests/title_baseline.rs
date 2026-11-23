//! The title-capture receive contract (`docs/title-capture.md` §6 in the
//! source experiment, now the manox leg's kernel baseline): what the
//! headless ghostty-vt interpreter does with OSC 0/2 sequences a program
//! writes into Bash output.
//!
//! These pin the KERNEL's behaviour only — sanitizing (trim, empty-title
//! fallback) lives in `agent_ui::title_store`. The interpreter is the
//! terminal world's existing one; there is no bespoke scanner to keep in
//! sync with these rows.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use steer_ghostty::Terminal;

/// A headless terminal plus the count of title effects it fired.
fn headless() -> (Terminal, Arc<AtomicUsize>) {
    let mut term = Terminal::new(80, 24).expect("headless terminal");
    let events = Arc::new(AtomicUsize::new(0));
    term.set_on_title({
        let events = events.clone();
        Box::new(move || {
            events.fetch_add(1, Ordering::SeqCst);
        })
    });
    (term, events)
}

fn title(term: &Terminal) -> Option<String> {
    term.title()
}

#[test]
fn osc2_terminated_by_bel_and_st() {
    let (term, events) = headless();
    // BEL (`\a`) terminator.
    term.write(b"\x1b]2;first title\x07");
    assert_eq!(title(&term).as_deref(), Some("first title"));
    // ST (`ESC \`) terminator.
    term.write(b"\x1b]2;second title\x1b\\");
    assert_eq!(title(&term).as_deref(), Some("second title"));
    assert_eq!(events.load(Ordering::SeqCst), 2);
}

#[test]
fn osc0_sets_the_same_single_title() {
    let (term, _events) = headless();
    term.write(b"\x1b]0;icon and title\x07");
    // No icon/title split: one string.
    assert_eq!(title(&term).as_deref(), Some("icon and title"));
}

#[test]
fn sequences_reassemble_across_arbitrary_byte_splits() {
    let (term, events) = headless();
    let stream = b"\x1b]2;split everywhere\x07";
    for byte in stream {
        term.write(std::slice::from_ref(byte));
    }
    assert_eq!(title(&term).as_deref(), Some("split everywhere"));
    assert_eq!(events.load(Ordering::SeqCst), 1);
}

#[test]
fn utf8_payload_survives_cuts_inside_a_character() {
    let (term, _events) = headless();
    let stream = "\u{1b}]2;终端标题 fix\x07".as_bytes();
    // `ESC ] 2 ;` is 4 bytes; the first cut lands MID-`终` (after 2 of its 3
    // UTF-8 bytes) and the second MID-`标` — no chunk ends on a codepoint
    // boundary, and the parser must still assemble the whole payload.
    term.write(&stream[..6]);
    term.write(&stream[6..12]);
    term.write(&stream[12..]);
    assert_eq!(title(&term).as_deref(), Some("终端标题 fix"));
}

#[test]
fn utf8_payload_survives_a_cut_on_a_character_boundary() {
    let (term, _events) = headless();
    let stream = "\u{1b}]2;终端标题 fix\x07".as_bytes();
    // The sibling cut: `stream[..7]` ends exactly at `终`'s last byte —
    // chunks that DO split on codepoint boundaries reassemble the same way.
    term.write(&stream[..7]);
    term.write(&stream[7..]);
    assert_eq!(title(&term).as_deref(), Some("终端标题 fix"));
}

#[test]
fn titles_land_on_the_alt_screen() {
    let (term, _events) = headless();
    term.write(b"\x1b[?1049h");
    term.write(b"\x1b]2;full screen title\x07");
    assert_eq!(title(&term).as_deref(), Some("full screen title"));
}

#[test]
fn an_empty_sequence_reports_an_empty_title() {
    let (term, events) = headless();
    term.write(b"\x1b]2;before\x07");
    term.write(b"\x1b]2;\x07");
    // The event still fires; the projection decides the meaning (an empty
    // program title falls back to the session name).
    assert_eq!(title(&term).as_deref(), Some(""));
    assert_eq!(events.load(Ordering::SeqCst), 2);
}

#[test]
fn an_over_long_title_is_truncated_not_dropped() {
    let (term, _events) = headless();
    let long = "a".repeat(2000);
    term.write(format!("\x1b]2;{long}\x07").as_bytes());
    let title = title(&term).expect("an over-long title still lands");
    assert!(!title.is_empty());
    assert!(title.len() <= 1024, "kernel cap exceeded: {}", title.len());
    assert!(long.starts_with(&title));
}

#[test]
fn a_reset_carries_the_whole_current_title() {
    let (term, _events) = headless();
    term.write(b"\x1b]2;one\x07");
    term.write(b"\x1b]2;two, complete\x07");
    // Full replacement, never an increment on the previous value.
    assert_eq!(title(&term).as_deref(), Some("two, complete"));
}

#[test]
fn a_c1_terminator_does_not_close_the_sequence() {
    let (term, _events) = headless();
    // Ghostty treats 0x9C as payload in UTF-8 mode: the sequence is dropped
    // and leaves no residue, and the next sequence parses from scratch.
    term.write(b"\x1b]2;abandoned\x9c");
    term.write(b"\x1b]2;recovered\x07");
    assert_eq!(title(&term).as_deref(), Some("recovered"));
}
