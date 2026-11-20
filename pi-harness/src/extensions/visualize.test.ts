/**
 * Unit tests for A3's visualize tool: the path guard and the result shape.
 * The fake ExtensionAPI checks the registration contract without a session.
 */

import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { checkWorkspaceFile, visualizeExtension, visualizeResult, VISUALIZE_TOOL_NAME } from "./visualize";

describe("visualize workspace guard", () => {
  let root: string;
  let cwd: string;

  function setup() {
    root = mkdtempSync(join(tmpdir(), "zu-visualize-"));
    cwd = join(root, "workspace");
    mkdirSync(cwd, { recursive: true });
    mkdirSync(join(cwd, "dir"), { recursive: true });
    mkdirSync(join(cwd, ".git"), { recursive: true });
    writeFileSync(join(cwd, "mockup.html"), "<h1>Hi</h1>");
    writeFileSync(join(cwd, "before.md"), "before");
    writeFileSync(join(cwd, "report.md"), "after");
    writeFileSync(join(cwd, ".git", "config"), "gitdir: x");
    writeFileSync(join(root, "outside.txt"), "outside");
    return root;
  }

  const cleanup = (value: string) => rmSync(value, { recursive: true, force: true });

  test("accepts regular files, including extensions and hidden workspace dirs", () => {
    const value = setup();
    try {
      expect(checkWorkspaceFile(cwd, "mockup.html")).toEqual({
        ok: true,
        abs: resolve(cwd, "mockup.html"),
      });
      expect(checkWorkspaceFile(cwd, "dir/../report.md").ok).toBe(true);
    } finally {
      cleanup(value);
    }
  });

  test("rejects absolute paths, .. escapes, missing files, directories and .git", () => {
    const value = setup();
    try {
      expect(checkWorkspaceFile(cwd, join(cwd, "mockup.html")).ok).toBe(false);
      expect(checkWorkspaceFile(cwd, "../outside.txt").ok).toBe(false);
      expect(checkWorkspaceFile(cwd, "missing.md").ok).toBe(false);
      expect(checkWorkspaceFile(cwd, "dir").ok).toBe(false);
      expect(checkWorkspaceFile(cwd, ".git/config").ok).toBe(false);
    } finally {
      cleanup(value);
    }
  });

  test("accepts files when cwd itself is reached through a symlink", () => {
    const value = setup();
    const link = join(root, "workspace-link");
    try {
      symlinkSync(cwd, link);
      expect(checkWorkspaceFile(link, "report.md")).toEqual({
        ok: true,
        abs: resolve(link, "report.md"),
      });
      expect(checkWorkspaceFile(link, "../outside.txt").ok).toBe(false);
    } finally {
      cleanup(value);
    }
  });

  test("accepts a symlink whose target stays inside the workspace", () => {
    const value = setup();
    try {
      symlinkSync(join(cwd, "report.md"), join(cwd, "report-link.md"));
      expect(checkWorkspaceFile(cwd, "report-link.md").ok).toBe(true);
    } finally {
      cleanup(value);
    }
  });

  test("rejects a symlink that escapes the workspace", () => {
    const value = setup();
    try {
      symlinkSync(join(root, "outside.txt"), join(cwd, "link.txt"));
      expect(checkWorkspaceFile(cwd, "link.txt").ok).toBe(false);
    } finally {
      cleanup(value);
    }
  });

  test("returns the fallback text, details and caption", () => {
    const value = setup();
    try {
      expect(visualizeResult(cwd, { type: "file", path: "report.md" })).toEqual({
        content: [{ type: "text", text: "Preview: report.md" }],
        details: { kind: "file", path: "report.md" },
      });
      expect(visualizeResult(cwd, { type: "file", path: "report.md", caption: "final" })).toEqual({
        content: [{ type: "text", text: "Preview: report.md — final" }],
        details: { kind: "file", path: "report.md", caption: "final" },
      });
      expect(visualizeResult(cwd, { type: "file", path: "report.md", before: "before.md" })).toEqual({
        content: [{ type: "text", text: "Diff: before.md → report.md" }],
        details: { kind: "file", path: "report.md", before: "before.md" },
      });
    } finally {
      cleanup(value);
    }
  });

  test("rejects invalid paths and same-path diffs as structured errors", () => {
    const value = setup();
    try {
      const invalid = visualizeResult(cwd, { type: "file", path: "../outside.txt" });
      expect(invalid.isError).toBe(true);
      expect(invalid.details).toEqual({ kind: "error" });
      const same = visualizeResult(cwd, { type: "file", path: "report.md", before: "report.md" });
      expect(same.isError).toBe(true);
      expect(same.content[0].text).toContain("different files");
    } finally {
      cleanup(value);
    }
  });

  test("the extension registers the captain-only tool name", async () => {
    const registered: any[] = [];
    const pi = { registerTool: (tool: any) => registered.push(tool) } as unknown as ExtensionAPI;
    visualizeExtension()(undefined)(pi);
    expect(registered.map((tool) => tool.name)).toEqual([VISUALIZE_TOOL_NAME]);

    const value = setup();
    try {
      const result = await registered[0].execute(
        "call-1",
        { type: "file", path: "report.md", caption: "draft" },
        undefined,
        undefined,
        { cwd },
      );
      expect(result.details).toEqual({ kind: "file", path: "report.md", caption: "draft" });
    } finally {
      cleanup(value);
    }
  });
});
