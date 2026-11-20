/**
 * Project trust, resolved the way the official CLI resolves it, with one
 * addition: when the answer is "ask" and an ACP client is attached, the
 * question is a standard `elicitation/create` form instead of a TUI prompt.
 *
 * Precedence (CLI `dist/core/project-trust.js`):
 *   1. no trust-requiring resources → the saved decision, else trusted
 *   2. an extension `project_trust` handler (yes/no/undecided; may remember)
 *   3. a saved decision in `~/.pi/agent/trust.json`
 *   4. `defaultProjectTrust`: always / never / ask
 *   5. ask without a client → untrusted (the headless behavior)
 *
 * The CLI's `emitProjectTrustEvent` is internal, but its inputs are public:
 * the pre-trust `LoadExtensionsResult` carries each extension's handler map,
 * so the same first-yes/no-wins semantics are reproduced here.
 */

import {
  hasTrustRequiringProjectResources,
  ProjectTrustStore,
  SettingsManager,
} from "@earendil-works/pi-coding-agent";
import type {
  LoadExtensionsResult,
  ProjectTrustContext,
  ProjectTrustEvent,
  ProjectTrustEventResult,
  ProjectTrustHandler,
} from "@earendil-works/pi-coding-agent";

/** One client question; structurally ACP's `elicitation/create` params. */
export type TrustElicit = (request: {
  mode: "form";
  sessionId: string;
  message: string;
  requestedSchema: {
    type: "object";
    properties: Record<string, unknown>;
    required?: string[];
  };
}) => Promise<{ action?: string; content?: Record<string, unknown> }>;

export type TrustSource = "no-resources" | "extension" | "store" | "default" | "elicitation" | "no-ui";

export interface TrustDecision {
  /** The project has `.pi`/`.agents` resources that need a decision. */
  required: boolean;
  trusted: boolean;
  source: TrustSource;
}

export interface ResolveTrustOptions {
  cwd: string;
  agentDir: string;
  /** The loader's pre-trust extension set; project_trust handlers decide first. */
  extensionsResult?: LoadExtensionsResult;
  elicit?: TrustElicit;
  log?: (line: string) => void;
}

/**
 * Replicates `emitProjectTrustEvent` from the CLI runner with public types:
 * the first handler returning yes/no wins; undecided falls through, and a
 * handler error is logged and skipped. The event emitter itself is internal,
 * but the handler map on `LoadExtensionsResult` is public.
 */
async function extensionTrustDecision(
  options: ResolveTrustOptions,
): Promise<ProjectTrustEventResult | undefined> {
  const extensionsResult = options.extensionsResult;
  if (!extensionsResult) return undefined;
  const event: ProjectTrustEvent = { type: "project_trust", cwd: options.cwd };
  const context = trustContext(options);
  for (const extension of extensionsResult.extensions) {
    const handlers = extension.handlers.get("project_trust") as ProjectTrustHandler[] | undefined;
    for (const handler of handlers ?? []) {
      try {
        const result = await handler(event, context);
        if (result.trusted !== "undecided") return result;
      } catch (failure) {
        options.log?.(`extension ${extension.path} project_trust failed: ${String(failure)}`);
      }
    }
  }
  return undefined;
}

/**
 * Trust handlers run headless, like the CLI's non-interactive context: they
 * may decide programmatically, but the ACP dialog is the host's question, not
 * a general ExtensionUIContext. A handler that needs a UI returns undecided
 * and the fallback below asks the user.
 */
function trustContext(options: ResolveTrustOptions): ProjectTrustContext {
  return {
    cwd: options.cwd,
    mode: "rpc",
    hasUI: false,
    ui: {
      select: async () => undefined,
      confirm: async () => false,
      input: async () => undefined,
      notify: (message) => options.log?.(`extension trust: ${message}`),
    },
  };
}

/**
 * Decides whether `cwd`'s project resources may load. An accepted prompt is
 * remembered in pi's own trust store, so the official CLI sees the same
 * decision; a declined prompt is session-only.
 */
export async function resolveProjectTrust(options: ResolveTrustOptions): Promise<TrustDecision> {
  const { cwd, agentDir, elicit, log } = options;
  const store = new ProjectTrustStore(agentDir);
  const required = hasTrustRequiringProjectResources(cwd);
  const saved = store.get(cwd);

  if (!required) {
    return { required: false, trusted: saved ?? true, source: saved === null ? "no-resources" : "store" };
  }
  const decision = await extensionTrustDecision(options);
  if (decision) {
    const trusted = decision.trusted === "yes";
    if (decision.remember === true) store.set(cwd, trusted);
    return { required: true, trusted, source: "extension" };
  }
  if (saved !== null) {
    return { required: true, trusted: saved, source: "store" };
  }

  const bootstrap = SettingsManager.create(cwd, agentDir, { projectTrusted: false });
  const defaultTrust = bootstrap.getDefaultProjectTrust();
  if (defaultTrust === "always") return { required: true, trusted: true, source: "default" };
  if (defaultTrust === "never") return { required: true, trusted: false, source: "default" };

  if (!elicit) {
    log?.(`project ${cwd} needs trust; no client UI is attached, running untrusted`);
    return { required: true, trusted: false, source: "no-ui" };
  }

  try {
    const response = await elicit({
      // Trust is negotiated while `session/new` is still in flight; the
      // client's session id does not exist yet. The client dialog keys on
      // cwd, so an empty scope is honest rather than invented.
      mode: "form",
      sessionId: "",
      message: `Trust this project?\n\n${cwd}\n\nIts .pi settings, extensions, skills, packages and MCP servers will load.`,
      requestedSchema: {
        type: "object",
        properties: {
          trusted: {
            type: "boolean",
            title: "Trust this project",
            description: "Project-local resources can execute code and read credentials.",
          },
        },
        required: ["trusted"],
      },
    });
    const trusted = response.action === "accept" && response.content?.trusted === true;
    if (trusted) store.set(cwd, true);
    log?.(`project ${cwd} ${trusted ? "trusted" : "left untrusted"} by the client`);
    return { required: true, trusted, source: "elicitation" };
  } catch (failure) {
    log?.(`project trust question failed for ${cwd}: ${String(failure)}`);
    return { required: true, trusted: false, source: "no-ui" };
  }
}
