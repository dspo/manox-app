/**
 * The Bash tool's title tap (issue #73): a hosted session's Bash tool runs
 * on a pipe, so a tool's `printf '\033]2;t\007'` never reaches a terminal.
 * The host hands the harness a channel fd (`ZU_TITLE_FD`); the tap tees
 * every raw output chunk there, and the host's headless emulator is the
 * only interpreter. The tool's own result (stripped text, exit code) is
 * untouched — the tap is a pure observer.
 *
 * Injection point: `customTools` with `createBashToolDefinition`, which
 * overrides the built-in bash by name (definition registry and tool
 * registry both apply custom tools after builtins) while keeping the
 * renderer and the user's shell settings.
 */
import {
  createBashToolDefinition,
  createLocalBashOperations,
  type BashOperations,
  type ToolDefinition,
} from "@earendil-works/pi-coding-agent";
import { writeSync } from "node:fs";

/** The env var naming the fd the host handed this harness. */
const TITLE_FD_ENV = "ZU_TITLE_FD";

/** The channel's write end from an env: the inherited fd, or nowhere.
 *
 * The fd is non-blocking (the host opens it that way): a full pty buffer
 * returns EAGAIN and the tap **drops the rest** instead of blocking. That
 * is deliberate — a blocked `writeSync` here would stall the harness's
 * whole event loop (ACP frames included), and the title is decorative.
 * Normally the host's drain keeps up and nothing drops.
 */
function channelWrite(env: NodeJS.ProcessEnv): (data: Buffer) => void {
  const raw = env[TITLE_FD_ENV];
  const fd = raw === undefined ? Number.NaN : Number(raw);
  if (!Number.isInteger(fd) || fd < 0) return () => {};
  return (data: Buffer) => {
    try {
      let offset = 0;
      while (offset < data.length) {
        offset += writeSync(fd, data.subarray(offset));
      }
    } catch (failure) {
      // EAGAIN (buffer full) or a closed channel: drop the rest. The
      // next title wins; the tool's own output and exit code are
      // untouched.
      void failure;
    }
  };
}

/** Wraps operations so every raw chunk also feeds the title channel. */
export function tapBashOperations(
  inner: BashOperations,
  write: (data: Buffer) => void,
): BashOperations {
  return {
    exec: (command, cwd, options) =>
      inner.exec(command, cwd, {
        ...options,
        onData: (data: Buffer) => {
          write(data);
          options.onData(data);
        },
      }),
  };
}

/** The shape of a settings manager the tap needs (pi's, narrowed). */
export interface ShellSettings {
  getShellPath?(): string | undefined;
  getShellCommandPrefix?(): string | undefined;
}

/** The bash tool whose raw output feeds the title channel (issue #73). */
export function tappedBashTool(
  cwd: string,
  settings: ShellSettings | undefined,
  env: NodeJS.ProcessEnv = process.env,
): ToolDefinition {
  const shellPath = settings?.getShellPath?.();
  const commandPrefix = settings?.getShellCommandPrefix?.();
  const inner = createLocalBashOperations(shellPath ? { shellPath } : {});
  return createBashToolDefinition(cwd, {
    operations: tapBashOperations(inner, channelWrite(env)),
    ...(shellPath ? { shellPath } : {}),
    ...(commandPrefix ? { commandPrefix } : {}),
  }) as unknown as ToolDefinition;
}
