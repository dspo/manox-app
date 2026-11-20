/**
 * Unit tests for the artifacts guard: the pure path logic, the write-tool
 * guard over a fake ExtensionAPI, and the lazy `.zu/artifacts/` setup.
 * No pi, no session — every fixture is a real temp directory.
 */

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import {
  ARTIFACTS_RULE,
  artifactsDir,
  artifactsExtension,
  ensureArtifactsDir,
  temporaryWriteTarget,
  writeInputTarget,
} from "./artifacts";

// ---------------------------------------------------------------- fixtures

describe("artifacts guard", () => {
  /**
   * Fresh base per test, under the repo so it sits under no temp root
   * (system temp would make the workspace itself "temporary"). `ws/` is
   * the session cwd; `tmp/` the fixture temp root, named only by the env
   * the context passes (and by os.tmpdir() while that test patches it).
   */
  let base: string;
  let workspace: string;
  let tempRoot: string;

  beforeEach(() => {
    mkdirSync(join(import.meta.dir, ".tmp-artifacts"), { recursive: true });
    base = mkdtempSync(join(import.meta.dir, ".tmp-artifacts", "case-"));
    workspace = join(base, "ws");
    tempRoot = join(base, "tmp");
    mkdirSync(workspace, { recursive: true });
    mkdirSync(tempRoot, { recursive: true });
  });

  afterEach(() => {
    rmSync(base, { recursive: true, force: true });
  });

  /** The context under test: only `env.TMPDIR` names the fixture temp root. */
  function context(env: Pick<NodeJS.ProcessEnv, "TMPDIR" | "TEMP" | "TMP"> = { TMPDIR: tempRoot }) {
    return { cwd: workspace, env, home: join(base, "home") };
  }

  // ------------------------------------------------ temporaryWriteTarget

  describe("temporaryWriteTarget", () => {
    test("a new file under the literal /tmp root hits", () => {
      expect(temporaryWriteTarget({ path: "/tmp/new.md" }, context())).toBe("/tmp/new.md");
    });

    test("a new file under os.tmpdir() hits", () => {
      // Isolate the os.tmpdir() contribution: the context's env trio is
      // empty, so the only way the target matches is the os root.
      const saved = process.env.TMPDIR;
      process.env.TMPDIR = tempRoot;
      try {
        const target = join(tmpdir(), "x.md");
        expect(temporaryWriteTarget({ path: target }, context({ TMPDIR: "", TEMP: "", TMP: "" }))).toBe(target);
      } finally {
        if (saved === undefined) delete process.env.TMPDIR;
        else process.env.TMPDIR = saved;
      }
    });

    test("a new file under env.TMPDIR hits", () => {
      expect(temporaryWriteTarget({ path: join(tempRoot, "x.md") }, context())).toBe(join(tempRoot, "x.md"));
    });

    test("editing an existing temp file is allowed (null)", () => {
      const existing = join(tempRoot, "existing.md");
      writeFileSync(existing, "keep");
      expect(temporaryWriteTarget({ path: existing }, context())).toBeNull();
    });

    test("a plain workspace path is allowed (null)", () => {
      expect(temporaryWriteTarget({ path: join(workspace, "report.md") }, context())).toBeNull();
    });

    test("a new file under .zu/artifacts/ is allowed even when cwd sits in temp", () => {
      const cwdInTemp = join(tempRoot, "project");
      mkdirSync(cwdInTemp, { recursive: true });
      const target = join(cwdInTemp, ".zu", "artifacts", "x.md");
      expect(
        temporaryWriteTarget({ path: target }, { cwd: cwdInTemp, env: { TMPDIR: tempRoot }, home: join(base, "home") }),
      ).toBeNull();
    });

    test("~/ expands to the injected home; home outside temp means allowed", () => {
      expect(writeInputTarget({ path: "~/x.md" }, context())).toBe(join(base, "home", "x.md"));
      expect(writeInputTarget({ path: "~" }, context())).toBe(join(base, "home"));
      // The expanded home is outside every temp root: allowed, not blocked.
      expect(temporaryWriteTarget({ path: "~/x.md" }, context())).toBeNull();
    });

    test("a home under a temp root still hits after expansion", () => {
      const home = join(tempRoot, "u");
      expect(
        temporaryWriteTarget({ path: "~/x.md" }, { cwd: workspace, env: { TMPDIR: tempRoot }, home }),
      ).toBe(join(home, "x.md"));
    });

    test("a relative path resolves against cwd and hits", () => {
      expect(temporaryWriteTarget({ path: "../tmp/new.md" }, context())).toBe(join(tempRoot, "new.md"));
    });

    test("a symlinked parent resolving into temp hits with the real path", () => {
      const link = join(workspace, "link");
      symlinkSync(tempRoot, link);
      const literal = join(link, "new.md");
      expect(temporaryWriteTarget({ path: literal }, context())).toBe(join(tempRoot, "new.md"));
    });

    test("a symlinked temp root blocks both spellings (canonical and lexical)", () => {
      // The root itself is reached through a symlink (macOS: /tmp is a
      // symlink to /private/tmp, and tools print the canonical spelling).
      // Whichever spelling the model writes must hit: the lexical form via
      // the env root, the canonical form via the root's realpath.
      const real = join(base, "real");
      const link = join(base, "link");
      mkdirSync(real, { recursive: true });
      symlinkSync(real, link);
      const ctx = { cwd: workspace, env: { TMPDIR: link, TEMP: "", TMP: "" }, home: join(base, "home") };
      expect(temporaryWriteTarget({ path: join(link, "new.md") }, ctx)).toBe(join(link, "new.md"));
      expect(temporaryWriteTarget({ path: join(real, "new.md") }, ctx)).toBe(join(real, "new.md"));
    });

    test("a parent symlink chain into a symlinked temp root hits via the canonical root", () => {
      // parent symlink -> symlinked root: the parent's realpath lands on the
      // root's realpath, which the roots set must carry (reviewer case ②:
      // realpathSync(parent) = canonical root, lexical roots alone miss it).
      const real = join(base, "real");
      const link = join(base, "link");
      mkdirSync(real, { recursive: true });
      symlinkSync(real, link);
      symlinkSync(link, join(workspace, "chain"));
      const ctx = { cwd: workspace, env: { TMPDIR: link, TEMP: "", TMP: "" }, home: join(base, "home") };
      expect(temporaryWriteTarget({ path: join(workspace, "chain", "new.md") }, ctx)).toBe(join(real, "new.md"));
    });

    test("a symlinked parent outside temp stays allowed", () => {
      const other = join(base, "other");
      mkdirSync(other, { recursive: true });
      const link = join(workspace, "link");
      symlinkSync(other, link);
      expect(temporaryWriteTarget({ path: join(link, "new.md") }, context())).toBeNull();
    });

    test("a missing parent directory is judged literally", () => {
      const nested = join(workspace, "no-such-dir", "new.md");
      expect(temporaryWriteTarget({ path: nested }, context())).toBeNull();
    });

    test("no usable path means no intervention", () => {
      expect(temporaryWriteTarget({}, context())).toBeNull();
      expect(temporaryWriteTarget({ path: "" }, context())).toBeNull();
      expect(temporaryWriteTarget({ path: 42 }, context())).toBeNull();
    });

    test("file_path is accepted as an alias for path", () => {
      expect(temporaryWriteTarget({ file_path: "/tmp/new.md" }, context())).toBe("/tmp/new.md");
      expect(writeInputTarget({ file_path: "a.md" }, context())).toBe(join(workspace, "a.md"));
    });

    test("an empty env var trio changes nothing", () => {
      expect(temporaryWriteTarget({ path: "/tmp/new.md" }, context({ TMPDIR: "", TEMP: "", TMP: "" }))).toBe(
        "/tmp/new.md",
      );
    });
  });

  // ------------------------------------------------------ artifacts dir

  describe("ensureArtifactsDir", () => {
    test("first call creates the tree and both self-ignore files", () => {
      ensureArtifactsDir(workspace);
      expect(existsSync(artifactsDir(workspace))).toBe(true);
      expect(readFileSync(join(workspace, ".zu", ".gitignore"), "utf8")).toBe("*\n");
      expect(readFileSync(join(artifactsDir(workspace), ".gitignore"), "utf8")).toBe("*\n");
    });

    test("is idempotent and never overwrites an existing .gitignore", () => {
      const zu = join(workspace, ".zu");
      mkdirSync(zu, { recursive: true });
      writeFileSync(join(zu, ".gitignore"), "keep\n");
      ensureArtifactsDir(workspace);
      ensureArtifactsDir(workspace);
      expect(readFileSync(join(zu, ".gitignore"), "utf8")).toBe("keep\n");
      expect(readFileSync(join(artifactsDir(workspace), ".gitignore"), "utf8")).toBe("*\n");
    });
  });

  // ------------------------------------------------- artifactsExtension

  describe("artifactsExtension", () => {
    /** Fake ExtensionAPI that records `on` registrations per event name. */
    function fakePi() {
      const registered = new Map<string, Array<(...args: unknown[]) => unknown>>();
      const pi = {
        on(event: string, handler: (...args: unknown[]) => unknown) {
          const list = registered.get(event) ?? [];
          list.push(handler);
          registered.set(event, list);
          return () => {
            registered.set(event, (registered.get(event) ?? []).filter((h) => h !== handler));
          };
        },
      };
      return { pi: pi as unknown as ExtensionAPI, registered };
    }

    function writeEvent(input: unknown) {
      return { type: "tool_call", toolCallId: "t1", toolName: "write", input };
    }

    test("registers exactly one tool_call handler and nothing else", () => {
      const { pi, registered } = fakePi();
      artifactsExtension()(pi);
      expect([...registered.keys()]).toEqual(["tool_call"]);
      expect(registered.get("tool_call")).toHaveLength(1);
    });

    test("blocks a new temp file with a reason naming .zu/artifacts, and logs", async () => {
      // The guard reads the real process env, so point TMPDIR at the fixture
      // root for the duration (save/restore) and keep the context env empty:
      // the hit can only come from the env the handler supplies itself.
      const saved = process.env.TMPDIR;
      process.env.TMPDIR = tempRoot;
      try {
        const lines: string[] = [];
        const { pi, registered } = fakePi();
        artifactsExtension((line) => lines.push(line))(pi);
        const handler = registered.get("tool_call")![0] as (e: unknown, ctx: unknown) => Promise<unknown>;
        const target = join(tempRoot, "report.md");
        const result = await handler(writeEvent({ path: target, content: "" }), { cwd: workspace });
        expect(result).toEqual({ block: true, reason: expect.stringContaining(".zu/artifacts") });
        expect((result as { reason: string }).reason).toContain(target);
        expect(lines).toHaveLength(1);
      } finally {
        if (saved === undefined) delete process.env.TMPDIR;
        else process.env.TMPDIR = saved;
      }
    });

    test("lets a workspace write through without a return value", async () => {
      const { pi, registered } = fakePi();
      artifactsExtension()(pi);
      const handler = registered.get("tool_call")![0] as (e: unknown, ctx: unknown) => Promise<unknown>;
      const result = await handler(writeEvent({ path: join(workspace, "report.md"), content: "" }), {
        cwd: workspace,
      });
      expect(result).toBeUndefined();
    });

    test("lets an existing temp file through (edit, not a new deliverable)", async () => {
      const existing = join(tempRoot, "existing.md");
      writeFileSync(existing, "keep");
      const { pi, registered } = fakePi();
      artifactsExtension()(pi);
      const handler = registered.get("tool_call")![0] as (e: unknown, ctx: unknown) => Promise<unknown>;
      expect(await handler(writeEvent({ path: existing, content: "patch" }), { cwd: workspace })).toBeUndefined();
    });

    test("an allowed write under .zu/artifacts/ lazily creates the tree", async () => {
      const { pi, registered } = fakePi();
      artifactsExtension()(pi);
      const handler = registered.get("tool_call")![0] as (e: unknown, ctx: unknown) => Promise<unknown>;
      const target = join(artifactsDir(workspace), "report.md");
      expect(await handler(writeEvent({ path: target, content: "" }), { cwd: workspace })).toBeUndefined();
      expect(readFileSync(join(workspace, ".zu", ".gitignore"), "utf8")).toBe("*\n");
      expect(existsSync(join(artifactsDir(workspace), ".gitignore"))).toBe(true);
    });

    test("ignores every tool except write", async () => {
      const { pi, registered } = fakePi();
      artifactsExtension()(pi);
      const handler = registered.get("tool_call")![0] as (e: unknown, ctx: unknown) => Promise<unknown>;
      const bash = { type: "tool_call", toolCallId: "t2", toolName: "bash", input: { command: "echo hi > /tmp/x" } };
      expect(await handler(bash, { cwd: workspace })).toBeUndefined();
    });
  });

  // ------------------------------------------------------- prompt rule

  describe("ARTIFACTS_RULE", () => {
    test("is a non-empty module-level constant mentioning the workspace tree", () => {
      expect(typeof ARTIFACTS_RULE).toBe("string");
      expect(ARTIFACTS_RULE.length).toBeGreaterThan(0);
      expect(ARTIFACTS_RULE).toContain(".zu/artifacts/");
    });
  });
});
