/**
 * The artifacts guard (A4): deliverables outlive the conversation.
 *
 * Sessions are durable assets in `~/.pi/agent/sessions/`, but anything the
 * model writes to `/tmp` dies with the temp cleaner. Two small pieces make
 * "don't write /tmp" a product behavior:
 *
 * 1. a byte-stable prompt section (`zu-artifacts`) telling the model where
 *    deliverables belong — `.zu/artifacts/` in the workspace;
 * 2. a `tool_call` guard on pi's built-in `write` tool that blocks creating
 *    a *new* file under a system temp root and names the workspace
 *    alternative instead. Editing an existing temp file is allowed (that is
 *    an edit, not a new deliverable), and bash redirections are explicitly
 *    out of scope — the rule covers those, the guard does not pretend to.
 *
 * The first allowed write under `.zu/artifacts/` creates the directory and
 * its self-ignore files (`.zu/.gitignore`, `.zu/artifacts/.gitignore`, both
 * `*\n`), so the workspace owner gitignores the whole tree without us
 * touching the project's own `.gitignore`. Nothing is created at session
 * start: read-only checkouts must stay untouched until a write actually
 * lands there.
 */

import { existsSync, mkdirSync, realpathSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, join, resolve, sep } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

/** The workspace directory Zu owns (relative to the session cwd). */
export const ARTIFACTS_DIRNAME = ".zu";
/** The deliverables subtree inside `.zu/`. */
export const ARTIFACTS_SUBDIR = "artifacts";

/** `<cwd>/.zu/artifacts` — where deliverables should land. */
export function artifactsDir(cwd: string): string {
  return join(cwd, ARTIFACTS_DIRNAME, ARTIFACTS_SUBDIR);
}

/**
 * The captain's deliverables rule, byte-stable like every prompt section.
 * Module-level const on purpose: it rides every captain request, and any
 * edit (or interpolation) invalidates the provider prefix cache.
 */
export const ARTIFACTS_RULE =
  "Anything the user should still have after this conversation — reports, charts, mockups — belongs under `.zu/artifacts/` in the workspace. Do not create new files in /tmp or any system temp directory. Only the user removes or moves files out of `.zu/artifacts/`.";

/** pi `write` input as we read it: 1.5.0's schema is `path`; `file_path` is
 * tolerated as an alias so a rename can never silently disarm the guard. */
export interface WriteTargetInput {
  path?: unknown;
  file_path?: unknown;
}

/** Everything the pure path logic may consult; injectable for tests. */
export interface WriteTargetContext {
  cwd: string;
  env: Pick<NodeJS.ProcessEnv, "TMPDIR" | "TEMP" | "TMP">;
  home: string;
}

/** The first string-valued field, or null when neither carries one. */
function inputPath(input: WriteTargetInput): string | null {
  for (const value of [input.path, input.file_path]) {
    if (typeof value === "string" && value.length > 0) return value;
  }
  return null;
}

/** Leading-`~` expansion only; `~user` forms stay literal. */
function expandHome(raw: string, home: string): string {
  if (raw === "~") return home;
  if (raw.startsWith("~/")) return join(home, raw.slice(2));
  return raw;
}

/** `target === root || target.startsWith(root + sep)`, the boundary-safe way. */
function under(root: string, target: string): boolean {
  return target === root || target.startsWith(root + sep);
}

/** The temp-root candidate set: os.tmpdir(), the literal roots, and the env
 * trio. Roots get both spellings: `resolve()` is lexical (it does not follow
 * symlinks), and the canonical spelling differs on macOS — `/tmp` and
 * `/var/tmp` are symlinks (`/tmp → /private/tmp`), so a write of the
 * canonical form (`/private/tmp/new.md`, what bash output and `realpath`
 * print) must hit too. Every root that exists on disk therefore also
 * contributes its `realpathSync()`; a missing root keeps only the lexical
 * form. Order irrelevant (any hit blocks). */
function tempRoots(context: WriteTargetContext): string[] {
  const candidates = [
    tmpdir(),
    "/tmp",
    "/var/tmp",
    context.env.TMPDIR,
    context.env.TEMP,
    context.env.TMP,
  ].filter((value): value is string => typeof value === "string" && value.length > 0);
  const roots = new Set<string>();
  for (const value of candidates) {
    const resolved = resolve(value);
    roots.add(resolved);
    try {
      roots.add(realpathSync(resolved));
    } catch {
      // Not on disk (or unreadable): the lexical form stays as the only one.
    }
  }
  return [...roots];
}

/**
 * Resolve the write input to its absolute target path — `~` expanded,
 * relative paths anchored at the session cwd — or null when the input
 * carries no usable path. Pure path logic; no existence check here.
 */
export function writeInputTarget(input: WriteTargetInput, context: WriteTargetContext): string | null {
  const raw = inputPath(input);
  if (!raw) return null;
  return resolve(context.cwd, expandHome(raw, context.home));
}

/**
 * The target of a `write` that would create a new file under a system temp
 * root, or null when the write needs no intervention:
 *
 * - no usable path, or the file already exists (editing a temp file is
 *   allowed — the deliverable already escaped once, not our call);
 * - the target is under `<cwd>/.zu/artifacts/` (the workspace wins, even
 *   when the session cwd itself sits in a temp directory);
 * - otherwise only when the target (literally, or through a symlinked
 *   parent resolved via `realpathSync`) equals a temp root or lives under
 *   one. Returns the path as resolved — real path when symlinks made the
 *   difference — so the block reason names where the file would land.
 */
export function temporaryWriteTarget(input: WriteTargetInput, context: WriteTargetContext): string | null {
  const target = writeInputTarget(input, context);
  if (!target || existsSync(target)) return null;

  // Rule 7: workspace artifacts always pass, before any temp check.
  if (under(artifactsDir(context.cwd), target)) return null;

  const roots = tempRoots(context);
  if (roots.some((root) => under(root, target))) return target;

  // Symlink avoidance: a literal miss can still land in temp through a
  // symlinked parent. Re-check once against the resolved parent; a missing
  // parent stays literal (we do not create directories to inspect them).
  const parent = dirname(target);
  if (existsSync(parent)) {
    try {
      const real = join(realpathSync(parent), basename(target));
      if (real !== target && roots.some((root) => under(root, real))) return real;
    } catch {
      // Unreadable parent (permissions, race): fall through to the literal verdict.
    }
  }
  return null;
}

/**
 * First-use setup for the artifacts tree: create `.zu/artifacts/` and write
 * `*\n` into both `.zu/.gitignore` and `.zu/artifacts/.gitignore` when
 * missing, so the whole tree stays out of git without touching the
 * project's own `.gitignore`. Idempotent; called only from the guard's
 * allow path, never at session start.
 */
export function ensureArtifactsDir(cwd: string): void {
  for (const dir of [join(cwd, ARTIFACTS_DIRNAME), artifactsDir(cwd)]) {
    const gitignore = join(dir, ".gitignore");
    if (existsSync(gitignore)) continue;
    mkdirSync(dir, { recursive: true });
    writeFileSync(gitignore, "*\n");
  }
}

/**
 * The guard extension: one `tool_call` handler on the `write` tool.
 *
 * - a new temp file is blocked with a reason that names the workspace
 *   alternative (the model rewrites the path itself; we never mutate input,
 *   retry, or move files);
 * - an allowed write under `.zu/artifacts/` lazily creates the tree
 *   (the guard's only side effect, idempotent);
 * - everything else passes untouched. The approval gate runs as its own
 *   handler and is unaffected; a read-only mode blocks `write` before this
 *   handler matters.
 */
export function artifactsExtension(log?: (line: string) => void) {
  return (pi: ExtensionAPI): void => {
    pi.on("tool_call", async (event, ctx) => {
      if (event.toolName !== "write") return;
      const cwd = ctx.cwd;
      const context: WriteTargetContext = {
        cwd,
        env: process.env as WriteTargetContext["env"],
        home: homedir(),
      };
      const target = temporaryWriteTarget(event.input, context);
      if (target) {
        const reason =
          `artifact-guard: ${target} is a new file in a temporary directory. ` +
          `Write it under ${artifactsDir(cwd)}/ instead (the workspace keeps it; /tmp does not).`;
        log?.(reason);
        return { block: true, reason };
      }
      // Allowed write: first use under the artifacts tree creates it.
      const allowed = writeInputTarget(event.input, context);
      if (allowed && under(artifactsDir(cwd), allowed)) ensureArtifactsDir(cwd);
      return undefined;
    });
  };
}
