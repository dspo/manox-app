/**
 * One Pi runtime, assembled the way the official CLI assembles it.
 *
 * `createAgentSessionServices` owns the order that matters: settings →
 * resource loader (with the trust resolver) → extension provider
 * registrations → model refresh. We supply the one process-wide
 * `ModelRuntime` (so every session and subagent shares auth/model state) and
 * the official built-in factories.
 *
 * `zu` extensions (permission gate, dispatch, registry entries) are inline
 * factories. They are appended after the built-ins, exactly where the CLI
 * puts caller-supplied factories — and then moved ahead of file extensions:
 * pi resolves a tool name by load order (first registration wins), while the
 * loader always appends inline factories after the packages it found in
 * `~/.pi`. Without the reorder, a user package could shadow `dispatch` or any
 * built-in extension's tool (see `hostExtensionsFirst`).
 */

import { join } from "node:path";
import {
  createAgentSessionServices,
  getAgentDir,
  ModelRuntime,
  SettingsManager,
  type AgentSessionServices,
  type Extension,
  type InlineExtension,
  type LoadExtensionsResult,
} from "@earendil-works/pi-coding-agent";
import { acpMcpExtension, type AcpMcpServer } from "./mcp";
import { OFFICIAL_BUILTIN_EXTENSIONS } from "./official";
import { resolveProjectTrust, type TrustElicit } from "./trust";

export interface PiServicesOptions {
  cwd: string;
  /** Defaults to `getAgentDir()`; explicit for tests and subagents. */
  agentDir?: string;
  /** Zu factories: the permission gate, dispatch, or a fixture. */
  extensionFactories?: InlineExtension[];
  /** ACP `session/new` servers (captain only; subagents have none). */
  mcpServers?: AcpMcpServer[];
  /** Known trust for a child session; when absent, trust is resolved. */
  trusted?: boolean;
  elicit?: TrustElicit;
  noExtensions?: boolean;
  additionalExtensionPaths?: string[];
  /** Skill roots beside pi's own discovery (built-in extensions own these). */
  additionalSkillPaths?: string[];
  appendSystemPrompt?: string[];
  log?: (line: string) => void;
}

export interface PiServicesResult {
  services: AgentSessionServices;
  trusted: boolean;
  agentDir: string;
}

const runtimes = new Map<string, Promise<ModelRuntime | undefined>>();

/** An inline factory, host- or caller-supplied: `<inline:name>` or `<inline:N>`. */
const HOST_INLINE_PATH = "<inline:";

/**
 * Host extensions win name conflicts.
 *
 * pi 1.5.0 keeps the first tool registered under a name
 * (`ExtensionRunner.getAllRegisteredTools`), and file extensions are loaded
 * before inline factories, so a `~/.pi` package registering `dispatch` would
 * win over the host's. The UI binds some tool names to surfaces, so the host's
 * inline extensions move to the front, preserving their relative order.
 *
 * This is lessons-learned code, not a guess: `precedence.test.ts` builds a
 * real package plus an inline factory that register one name and asserts the
 * inline one is what the session sees.
 */
function hostExtensionsFirst(result: LoadExtensionsResult): LoadExtensionsResult {
  const host: Extension[] = [];
  const rest: Extension[] = [];
  for (const extension of result.extensions) {
    (extension.path.startsWith(HOST_INLINE_PATH) ? host : rest).push(extension);
  }
  if (host.length === 0) return result;
  return { ...result, extensions: [...host, ...rest] };
}

/**
 * One ModelRuntime per agent dir for the process. Failure is logged and the
 * session falls back to the SDK's default runtime rather than refusing to
 * start: a model directory problem is not a process-level failure.
 */
export function sharedModelRuntime(agentDir: string, log: (line: string) => void = () => {}): Promise<ModelRuntime | undefined> {
  const existing = runtimes.get(agentDir);
  if (existing) return existing;
  const created = ModelRuntime.create({
    authPath: join(agentDir, "auth.json"),
    modelsPath: join(agentDir, "models.json"),
  }).catch((failure: unknown) => {
    log(`model runtime unavailable: ${String(failure)}`);
    return undefined;
  });
  runtimes.set(agentDir, created);
  return created;
}

/** Builds the services one captain or subagent session runs on. */
export async function createPiServices(options: PiServicesOptions): Promise<PiServicesResult> {
  const agentDir = options.agentDir ?? getAgentDir();
  const log = options.log ?? (() => {});
  const settingsManager = SettingsManager.create(options.cwd, agentDir, {
    projectTrusted: options.trusted ?? false,
  });
  const modelRuntime = await sharedModelRuntime(agentDir, log);

  const extensionFactories: InlineExtension[] = [
    ...OFFICIAL_BUILTIN_EXTENSIONS,
    ...(options.extensionFactories ?? []),
    ...(options.mcpServers && options.mcpServers.length > 0
      ? [acpMcpExtension(options.mcpServers, log)]
      : []),
  ];

  const services = await createAgentSessionServices({
    cwd: options.cwd,
    agentDir,
    settingsManager,
    ...(modelRuntime ? { modelRuntime } : {}),
    resourceLoaderOptions: {
      extensionFactories,
      extensionsOverride: hostExtensionsFirst,
      ...(options.noExtensions ? { noExtensions: true } : {}),
      ...(options.additionalSkillPaths && options.additionalSkillPaths.length > 0
        ? { additionalSkillPaths: options.additionalSkillPaths }
        : {}),
      ...(options.additionalExtensionPaths && options.additionalExtensionPaths.length > 0
        ? { additionalExtensionPaths: options.additionalExtensionPaths }
        : {}),
      ...(options.appendSystemPrompt && options.appendSystemPrompt.length > 0
        ? { appendSystemPrompt: options.appendSystemPrompt }
        : {}),
    },
    // A child session receives its captain's decision and must not ask again.
    ...(options.trusted === undefined
      ? {
          resourceLoaderReloadOptions: {
            resolveProjectTrust: async ({ extensionsResult }) =>
              (await resolveProjectTrust({
                cwd: options.cwd,
                agentDir,
                extensionsResult,
                ...(options.elicit ? { elicit: options.elicit } : {}),
                log,
              })).trusted,
          },
        }
      : {}),
  });

  for (const diagnostic of services.diagnostics) {
    log(`${diagnostic.type}: ${diagnostic.message}`);
  }
  return {
    services,
    trusted: services.settingsManager.isProjectTrusted(),
    agentDir,
  };
}
