/**
 * A3: `visualize` — show workspace files as artifacts (issue #15).
 *
 * The tool is file-first: the model writes a deliverable (mockup, report,
 * image, HTML) into the workspace and calls this tool with its relative
 * path. The tool never creates, moves, or renders a file. It returns a
 * bounded `details` reference that the client turns into a card with
 * open/compare actions.
 *
 * Clean room: built from the issue's SPEC and pi's public extension
 * contract only. ThinkRail's visualize source was not consulted.
 */

import { realpathSync, statSync } from "node:fs";
import { isAbsolute, relative, resolve, sep } from "node:path";
import { Type } from "typebox";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { ZuExtension } from "./registry";

export const VISUALIZE_TOOL_NAME = "visualize";

/** The only input shape v1 accepts. Later types are a schema change. */
const VisualizeParams = Type.Object({
  type: Type.Literal("file"),
  path: Type.String({
    description: "Workspace-relative path of the file to show.",
    minLength: 1,
  }),
  before: Type.Optional(
    Type.String({
      description: "Workspace-relative path of the before version for a diff.",
      minLength: 1,
    }),
  ),
  caption: Type.Optional(Type.String({ maxLength: 200 })),
});

export interface VisualizeInput {
  type: "file";
  path: string;
  before?: string;
  caption?: string;
}

export type WorkspaceFileCheck =
  | { ok: true; abs: string }
  | { ok: false; reason: string };

function insideWorkspace(cwd: string, abs: string): boolean {
  const rel = relative(cwd, abs);
  return rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel);
}

function underGit(abs: string): boolean {
  return relativeToRoot(abs).includes(".git");
}

/** A cwd can itself be reached through a symlink (macOS /var -> /private/var). */
function insideEitherRoot(root: string, realRoot: string, abs: string): boolean {
  return insideWorkspace(root, abs) || insideWorkspace(realRoot, abs);
}

function relativeToRoot(abs: string): string[] {
  return abs.split(/[\\/]/).filter(Boolean);
}

/**
 * Validate a workspace-relative input path. Rejects absolute paths, `..`
 * escapes, non-files, directories, `.git` entries, and symlinks whose real
 * target left the workspace. No extension allowlist: the renderer picks the
 * panel from the file's extension.
 */
export function checkWorkspaceFile(cwd: string, input: string): WorkspaceFileCheck {
  const root = resolve(cwd);
  let realRoot: string;
  try {
    realRoot = realpathSync(root);
  } catch {
    return { ok: false, reason: `cannot resolve workspace: ${root}` };
  }
  if (typeof input !== "string" || input.length === 0) {
    return { ok: false, reason: "path must be a non-empty string" };
  }
  if (isAbsolute(input)) {
    return { ok: false, reason: `path must be relative to the workspace (cwd: ${root})` };
  }
  const abs = resolve(root, input);
  if (abs === root || !insideWorkspace(root, abs)) {
    return { ok: false, reason: `path escapes the workspace (cwd: ${root})` };
  }
  if (underGit(abs) || underGit(realRoot === root ? abs : resolve(realRoot, relative(root, abs)))) {
    return { ok: false, reason: `path is inside .git (cwd: ${root})` };
  }
  let real: string;
  try {
    if (!statSync(abs).isFile()) {
      return { ok: false, reason: `path is not a regular file: ${input} (cwd: ${root})` };
    }
    real = realpathSync(abs);
  } catch {
    return { ok: false, reason: `file does not exist: ${input} (cwd: ${root})` };
  }
  if (!insideEitherRoot(root, realRoot, real) || underGit(real)) {
    return { ok: false, reason: `path escapes the workspace through a symlink (cwd: ${root})` };
  }
  return { ok: true, abs };
}

function errorResult(text: string) {
  return {
    content: [{ type: "text" as const, text }],
    details: { kind: "error" },
    isError: true as const,
  };
}

/** The model-facing fallback and the renderer-facing structured result. */
export function visualizeResult(
  cwd: string,
  input: VisualizeInput,
): {
  content: { type: "text"; text: string }[];
  details: Record<string, unknown>;
  isError?: true;
} {
  const target = checkWorkspaceFile(cwd, input.path);
  if (!target.ok) return errorResult(target.reason);
  if (input.before !== undefined) {
    if (input.before === input.path) {
      return errorResult(`before and path must be different files (cwd: ${resolve(cwd)})`);
    }
    const before = checkWorkspaceFile(cwd, input.before);
    if (!before.ok) return errorResult(before.reason);
    return {
      content: [{ type: "text", text: `Diff: ${input.before} → ${input.path}` }],
      details: {
        kind: "file",
        path: input.path,
        before: input.before,
        ...(input.caption === undefined ? {} : { caption: input.caption }),
      },
    };
  }
  const text = input.caption ? `Preview: ${input.path} — ${input.caption}` : `Preview: ${input.path}`;
  return {
    content: [{ type: "text", text }],
    details: {
      kind: "file",
      path: input.path,
      ...(input.caption === undefined ? {} : { caption: input.caption }),
    },
  };
}

/** The extension body. The cwd comes from pi's per-call context. */
export function visualizeExtension(): (settings: unknown) => (pi: ExtensionAPI) => void {
  return () => (pi) => {
    pi.registerTool({
      name: VISUALIZE_TOOL_NAME,
      label: "Visualize a workspace file",
      description:
        "Show the user a file they should look at: a mockup, image, report, HTML page, or a diff against a previous version. Give a workspace-relative path to an existing regular file; never paste ASCII art or inline HTML into the conversation. Write the file first with the write tool (deliverables belong under .zu/artifacts/). The client opens the file in a panel; this tool only returns a reference.",
      promptSnippet: "show a workspace file in a panel",
      parameters: VisualizeParams,
      executionMode: "sequential",
      async execute(_toolCallId: string, params: any, _signal: unknown, _onUpdate: unknown, ctx: any) {
        const cwd = String(ctx?.cwd ?? process.cwd());
        const input: VisualizeInput = {
          type: "file",
          path: String(params?.path ?? ""),
          ...(typeof params?.before === "string" ? { before: params.before } : {}),
          ...(typeof params?.caption === "string" ? { caption: params.caption } : {}),
        };
        return visualizeResult(cwd, input);
      },
    });
  };
}

/** B3 registry entry: the tool ships to captains only, disabled by name. */
export function visualizeExtensionEntry(): ZuExtension {
  return { name: VISUALIZE_TOOL_NAME, factory: visualizeExtension() };
}
