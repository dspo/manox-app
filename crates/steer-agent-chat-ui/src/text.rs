//! One-line display strings for the conversation column and the shell.

/// Collapse a display string onto ONE line: every whitespace run — the
/// newlines of a multi-line `bash` body or of a run's task label among
/// them — becomes a single space. `.truncate()` ellipsizes text that is
/// already one line, but a raw multi-line value still stretches its row to
/// as many lines as it had.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::one_line;

    #[test]
    fn one_line_collapses_whitespace_runs() {
        assert_eq!(one_line("cargo build"), "cargo build");
        assert_eq!(
            one_line("cat <<'EOF'\n  a\n  b\nEOF"),
            "cat <<'EOF' a b EOF"
        );
        assert_eq!(one_line("  \n \t "), "");
        assert_eq!(one_line("a\rb"), "a b");
    }
}
