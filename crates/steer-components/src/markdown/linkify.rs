//! Source-level link pre-processing: bare URLs and filesystem paths found in
//! plain markdown text are rewritten into explicit `[text](target)` links
//! before the source reaches `TextView`.
//!
//! The old renderer detected links post-parse on each block's display text;
//! upstream `TextView` has no autolink hook that can split one text node into
//! several, so the rewrite happens on the source instead. Detection must
//! therefore skip markdown structure itself: fenced code blocks, inline code
//! spans, existing `[..](..)` links, and `<scheme:..>` autolinks are masked
//! before detection runs.

use std::ops::Range;

use hyperlinks::{
    OverlaySpan, UrlKind, default_path_options, detect_paths, detect_urls, is_covered,
};

/// What a link target opens as, decided at click time from the target string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// An explicit markdown `[text](url)` or auto-detected bare URL.
    Url,
    /// A filesystem path auto-detected in plain text.
    FilePath,
}

/// Rewrite bare URLs and filesystem paths in `src` as explicit markdown links.
///
/// Idempotent by construction: the emitted `[text](target)` forms are masked on
/// the next pass, so feeding an already-linkified source through again is a
/// no-op. A span whose text carries `[`/`]` or whose target carries `<`/`>` is
/// left raw — rewriting either would produce markdown that does not re-parse
/// as the intended link.
pub fn linkify_markdown(src: &str) -> String {
    let masked = masked_ranges(src);
    let mut covered = masked;
    covered.sort_by_key(|r| r.start);

    let urls = detect_urls(src);
    let mut spans: Vec<OverlaySpan> = urls
        .iter()
        .filter(|span| !is_covered(span.range.clone(), &covered))
        .cloned()
        .collect();
    covered.extend(urls.iter().map(|s| s.range.clone()));
    covered.sort_by_key(|r| r.start);
    spans.extend(
        detect_paths(src, &default_path_options())
            .into_iter()
            .filter(|span| !is_covered(span.range.clone(), &covered)),
    );
    spans.sort_by_key(|s| s.range.start);

    if spans.is_empty() {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len());
    let mut cursor = 0usize;
    for span in spans {
        let range = span.range.clone();
        if range.start < cursor {
            continue;
        }
        let text = &src[range.clone()];
        if text.contains('[')
            || text.contains(']')
            || span.href.contains('<')
            || span.href.contains('>')
        {
            continue;
        }
        out.push_str(&src[cursor..range.start]);
        out.push('[');
        out.push_str(text);
        out.push_str("](");
        if span.href.contains(' ') || span.href.contains('(') || span.href.contains(')') {
            out.push('<');
            out.push_str(&span.href);
            out.push('>');
        } else {
            out.push_str(&span.href);
        }
        out.push(')');
        cursor = range.end;
    }
    out.push_str(&src[cursor..]);
    out
}

/// Classify a link target for click dispatch. URLs are recognized by the same
/// scheme set detection uses; everything else that reaches a click handler came
/// from path detection.
pub fn classify_target(target: &str) -> LinkKind {
    if detect_urls(target)
        .iter()
        .any(|s| s.kind == UrlKind::Url && s.href == target)
    {
        LinkKind::Url
    } else {
        LinkKind::FilePath
    }
}

/// Byte ranges of `src` where auto-detection must not run: fenced code blocks,
/// inline code spans, existing inline links / images, and `<scheme:..>`
/// autolinks. Indented (4-space) code blocks are not masked — they need block
/// context to tell apart from deeply indented prose, and a mis-linked path
/// inside one is a tolerable edge.
fn masked_ranges(src: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    ranges.extend(fenced_blocks(src));
    ranges.extend(inline_code_spans(src));
    ranges.extend(explicit_links(src));
    ranges.extend(autolinks(src));
    consolidate(&mut ranges);
    ranges
}

/// Merge overlapping / touching ranges into a sorted, disjoint list.
fn consolidate(ranges: &mut Vec<Range<usize>>) {
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges.drain(..) {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => {
                last.end = last.end.max(range.end);
            }
            _ => merged.push(range),
        }
    }
    *ranges = merged;
}

/// Fenced code blocks: an opening fence line (3+ backticks or tildes) through
/// the matching closing fence line (or end of input for an unterminated fence).
fn fenced_blocks(src: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut open: Option<(u8, usize, usize)> = None; // (fence char, fence len, start offset)
    let mut offset = 0usize;
    for line in src.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let fence_char = trimmed.as_bytes().first().copied();
        let fence_len = trimmed
            .bytes()
            .take_while(|&b| Some(b) == fence_char && (b == b'`' || b == b'~'))
            .count();
        let opens = fence_len >= 3 && matches!(fence_char, Some(b'`') | Some(b'~'));
        match open {
            None => {
                if opens {
                    open = Some((fence_char.unwrap(), fence_len, offset));
                }
            }
            Some((ch, len, start)) => {
                // A closing fence is the same char, at least as long, alone on
                // its line (info strings only appear on openers).
                if fence_char == Some(ch)
                    && fence_len >= len
                    && trimmed[fence_len..].trim_end().is_empty()
                {
                    ranges.push(start..offset + line.len());
                    open = None;
                }
            }
        }
        offset += line.len();
    }
    if let Some((_, _, start)) = open {
        ranges.push(start..src.len());
    }
    ranges
}

/// Inline code spans: a backtick run through its matching same-length run.
fn inline_code_spans(src: &str) -> Vec<Range<usize>> {
    let bytes = src.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let len = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            // Find the matching closing run of the same length after at least
            // one byte of content (CommonMark requires a code span to have a
            // closing delimiter).
            let mut j = i + len;
            let mut close = None;
            while j < bytes.len() {
                if bytes[j] == b'`' {
                    let run = bytes[j..].iter().take_while(|&&b| b == b'`').count();
                    if run == len {
                        close = Some(j + run);
                        break;
                    }
                    j += run;
                } else {
                    j += 1;
                }
            }
            match close {
                Some(end) => {
                    ranges.push(i..end);
                    i = end;
                }
                None => {
                    // Unterminated opener: mask the run itself so a stray
                    // backtick does not start path detection inside it.
                    ranges.push(i..i + len);
                    i += len;
                }
            }
        } else {
            i += 1;
        }
    }
    ranges
}

/// Inline links and images: optional `!`, a bracketed label, then a
/// parenthesized target. Reference-style links (`[a][b]`) are not masked —
/// rare in chat bodies, and a mis-detected path inside one degrades to a
/// plain-looking link label.
fn explicit_links(src: &str) -> Vec<Range<usize>> {
    let bytes = src.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'!' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            i += 1;
        }
        if bytes[i] != b'[' {
            i += 1;
            continue;
        }
        let Some(close_label) = matching_bracket(src, i) else {
            i += 1;
            continue;
        };
        if close_label + 1 < bytes.len()
            && bytes[close_label + 1] == b'('
            && let Some(close_target) = matching_paren(src, close_label + 1)
        {
            ranges.push(i..close_target + 1);
            i = close_target + 1;
            continue;
        }
        i = close_label + 1;
    }
    ranges
}

/// `<scheme://...>` autolinks.
fn autolinks(src: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut rest = src;
    let mut base = 0usize;
    while let Some(lt) = rest.find('<') {
        let after = &rest[lt + 1..];
        if let Some(gt) = after.find('>') {
            let inner = &after[..gt];
            let looks_like_autolink = hyperlinks::SCHEMES
                .iter()
                .any(|scheme| inner.starts_with(*scheme) && inner.len() > scheme.len() + 2);
            if looks_like_autolink {
                let start = base + lt;
                ranges.push(start..start + 1 + gt + 1);
            }
            base += lt + 1 + gt + 1;
            rest = &rest[lt + 1 + gt + 1..];
        } else {
            break;
        }
    }
    ranges
}

/// Index of the `]` matching the `[` at `open`, honoring backslash escapes and
/// nested brackets.
fn matching_bracket(src: &str, open: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Index of the `)` matching the `(` at `open`, honoring backslash escapes and
/// nesting.
fn matching_paren(src: &str, open: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_detects_bare_url() {
        assert_eq!(
            linkify_markdown("see https://example.com/x today"),
            "see [https://example.com/x](https://example.com/x) today"
        );
    }

    #[test]
    fn auto_detects_file_path_with_extension() {
        assert_eq!(
            linkify_markdown("edit crates/app/src/main.rs now"),
            "edit [crates/app/src/main.rs](crates/app/src/main.rs) now"
        );
    }

    #[test]
    fn explicit_link_is_not_double_linked() {
        let src = "[example](https://example.com)";
        assert_eq!(linkify_markdown(src), src);
    }

    #[test]
    fn url_inside_existing_link_label_is_left_alone() {
        let src = "[see https://example.com](https://example.com)";
        assert_eq!(linkify_markdown(src), src);
    }

    #[test]
    fn fenced_code_block_is_not_linkified() {
        let src = "```\ncargo build https://example.com\n```";
        assert_eq!(linkify_markdown(src), src);
    }

    #[test]
    fn inline_code_span_is_not_linkified() {
        assert_eq!(
            linkify_markdown("run `open https://example.com` please"),
            "run `open https://example.com` please"
        );
    }

    #[test]
    fn autolink_is_not_double_linked() {
        let src = "mail <https://example.com/a> here";
        assert_eq!(linkify_markdown(src), src);
    }

    #[test]
    fn paths_with_spaces_are_not_detected() {
        // The path detector stops at whitespace, so a path with a space is
        // left raw — the same boundary the old renderer's post-parse
        // detection had. The angle-bracket target escaping in `linkify`
        // stays as defense for any href detection may return with a space.
        assert_eq!(
            linkify_markdown("open /tmp/my file.txt next"),
            "open /tmp/my file.txt next"
        );
    }

    #[test]
    fn label_with_brackets_is_left_raw() {
        let src = "path /tmp/[weird].txt here";
        assert_eq!(linkify_markdown(src), src);
    }

    #[test]
    fn linkify_is_idempotent() {
        let once = linkify_markdown("see https://example.com/x and /tmp/a.rs");
        assert_eq!(linkify_markdown(&once), once);
    }

    #[test]
    fn classify_splits_urls_from_paths() {
        assert_eq!(classify_target("https://example.com"), LinkKind::Url);
        assert_eq!(
            classify_target("crates/app/src/main.rs"),
            LinkKind::FilePath
        );
    }
}
