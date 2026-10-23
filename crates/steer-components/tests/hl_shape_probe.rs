//! Property probe: `steer_code_highlighter` output is what gpui's run builder
//! consumes for code blocks (`Inline` + `text_runs`), and when no stream fade
//! is active that output reaches the builder UNMERGED — so it must itself be
//! sorted, non-overlapping, in bounds, and char-boundary aligned, or the app
//! aborts with `invalid text run`. Asserts that shape over CJK-heavy
//! tool-output shapes (ANSI SGR sequences, tree-sitter languages, diffs,
//! conflict markers).

use gpui_base::text::CodeBlock;
use steer_components::markdown::steer_code_highlighter;

fn assert_runnable_shape(
    label: &str,
    runs: &[(std::ops::Range<usize>, gpui::HighlightStyle)],
    code: &str,
) {
    let len = code.len();
    let mut prev_end = 0;
    for (ix, (range, _)) in runs.iter().enumerate() {
        assert!(
            range.start >= prev_end,
            "{label} run #{ix} overlaps or is out of order: {range:?} after end {prev_end} (code: {})",
            code.escape_default()
        );
        assert!(
            range.end <= len,
            "{label} run #{ix} out of bounds: {range:?} vs {len} bytes"
        );
        assert!(
            code.is_char_boundary(range.start)
                && (range.end == len || code.is_char_boundary(range.end)),
            "{label} run #{ix} splits a char: {range:?}"
        );
        prev_end = range.end;
    }
}

fn check(cx: &mut gpui::TestAppContext, label: &str, lang: &str, code: &str) {
    let runs = cx.update(|cx| {
        let hl = steer_code_highlighter(gpui_component::Theme::global(cx));
        hl(&CodeBlock::from_code(code, Some(lang)))
    });
    assert_runnable_shape(label, &runs, code);
}

#[gpui::test]
fn ansi_tool_output_with_cjk_stays_runnable(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);
    let samples: Vec<&str> = vec![
        // SGR colors around CJK, reset mid-line, cursor moves, wide chars.
        "\x1b[31m错误\x1b[0m：文件不存在\n",
        "\x1b[1;32m[OK]\x1b[0m 第一步完成：中文输出 ✓\n",
        "\x1b[2K\r进度：\x1b[33m42%\x1b[0m 完成",
        "混排\x1b[4m下划线中文\x1b[24m与\x1b[7m反显日本語\x1b[27m收尾",
        // 256-color and truecolor with CJK labels.
        "\x1b[38;5;196m红色中文\x1b[0m 与 \x1b[38;2;0;255;0m绿字中文\x1b[0m",
        // Truncated escape at the end (streaming cut mid-sequence).
        "输出中...\x1b[38;5;2",
        // CRLF and tabs interleaved with SGR.
        "a\r\nb\tc\x1b[35m紫中文\x1b[0m\r\n",
    ];
    for (ix, sample) in samples.iter().enumerate() {
        check(cx, &format!("ansi#{ix}"), "ansi", sample);
    }
}

#[gpui::test]
fn tree_sitter_languages_with_cjk_stay_runnable(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);
    let cases: Vec<(&str, &str)> = vec![
        (
            "rust",
            "fn 主函数() {\n    // 中文注释 with English\n    let s = \"字符串字面值\";\n    println!(\"{}\", s);\n}",
        ),
        (
            "python",
            "# 中文注释\ndef 函数(参数):\n    \"\"\"文档字符串中文\"\"\"\n    return f\"{参数} 结果\"",
        ),
        (
            "ts",
            "// 中文注释\nconst 变量: string = `模板${1}中文`;\nexport function 函数(): void {}",
        ),
        (
            "json",
            "{\n  \"键\": \"中文值\",\n  \"list\": [1, 2, 3],\n  \"nested\": {\"内\": true}\n}",
        ),
        (
            "html",
            "<div class=\"容器\">\n  <!-- 中文注释 -->\n  <p>段落中文</p>\n</div>",
        ),
        (
            "bash",
            "#!/bin/bash\n# 中文注释\necho \"变量=$HOME 中文\"\ngrep -r '模式' ./目录",
        ),
        ("ansi", "not an ansi fence but lang=ansi plain 中文 text"),
    ];
    for (ix, (lang, code)) in cases.iter().enumerate() {
        check(cx, &format!("ts#{ix}({lang})"), lang, code);
    }
}

#[gpui::test]
fn diff_and_conflict_with_cjk_stay_runnable(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);
    let diff = "diff --git a/中文.rs b/中文.rs\nindex 123..456 100644\n--- a/中文.rs\n+++ b/中文.rs\n@@ -1,3 +1,3 @@\n-删除的中文行\n+新增的中文行\n 上下文中文\n";
    check(cx, "diff", "diff", diff);
    let conflict = "<<<<<<< HEAD\n我们的中文修改\n=======\n他们的中文修改\n>>>>>>> branch\n";
    check(cx, "conflict", "text", conflict);
}

#[gpui::test]
fn crash_payload_lines_stay_runnable(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);
    // The exact lines captured in ~/Library/Logs/Steer/panic.log — the run
    // boundary that aborted landed mid-char inside 显 (45..48) and 额's tail.
    let line_a = "  MR!470 [merged] ->main 2026-10-09 docs(hua5): 额度接口按线上实测修订（路径 / 可用性 / 字段族三处不实）";
    let line_b = "  MR!464 [merged] ->main 2026-10-08 fix(helm): HPA 显式声明 behavior，缩容语义不再依赖控制面默认值";
    let line_c = " 10M\t/Users/chenzhongrun/projects/huayi/go-homework-s2/周志刚/test/conformance.test\tMach-O 64-bit executable arm64";
    let block = format!("{line_a}\n{line_b}\n{line_c}\n");
    for lang in ["bash", "text", "rust", "python", "ansi", "json", ""] {
        check(cx, &format!("payload({lang})"), lang, &block);
        check(
            cx,
            &format!("payload-a({lang})"),
            lang,
            &format!("{line_a}\n"),
        );
        check(
            cx,
            &format!("payload-b({lang})"),
            lang,
            &format!("{line_b}\n"),
        );
        check(
            cx,
            &format!("payload-c({lang})"),
            lang,
            &format!("{line_c}\n"),
        );
    }
}
