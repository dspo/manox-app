import { afterAll, describe, expect, test } from "bun:test";
import { closeSync, mkdtempSync, openSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  createLocalBashOperations,
  type BashOperations,
} from "@earendil-works/pi-coding-agent";
import { tapBashOperations, tappedBashTool } from "./title-tap";

/** Temp dirs to clean up at the end. */
const dirs: string[] = [];

/** Collects what the tap saw, in order. */
function collector(): { write(data: Buffer): void; seen: Buffer[] } {
  const seen: Buffer[] = [];
  return { write: (data) => void seen.push(data), seen };
}

/** Opens a channel file and points `ZU_TITLE_FD` at it, saving the old env. */
function openChannel(prefix: string): { dir: string; file: string; restore(): void } {
  const dir = mkdtempSync(join(tmpdir(), prefix));
  dirs.push(dir);
  const file = join(dir, "channel");
  const fd = openSync(file, "w");
  const saved = process.env.ZU_TITLE_FD;
  process.env.ZU_TITLE_FD = String(fd);
  return {
    dir,
    file,
    restore() {
      if (saved === undefined) delete process.env.ZU_TITLE_FD;
      else process.env.ZU_TITLE_FD = saved;
      closeSync(fd);
    },
  };
}

describe("title tap", () => {
  afterAll(() => {
    for (const dir of dirs) rmSync(dir, { recursive: true, force: true });
  });

  test("the tap feeds every raw chunk to the channel and keeps pi's stream", async () => {
    const tap = collector();
    const innerSeen: Buffer[] = [];
    const inner: BashOperations = {
      exec: async (command, cwd, options) => {
        expect(command).toBe("printf '\\033]2;t\\a'");
        expect(cwd).toBe("/tmp");
        options.onData(Buffer.from("\x1b]2;t\x07"));
        options.onData(Buffer.from("plain\n"));
        return { exitCode: 0 };
      },
    };
    const result = await tapBashOperations(inner, (data) => tap.write(data)).exec(
      "printf '\\033]2;t\\a'",
      "/tmp",
      { onData: (data: Buffer) => void innerSeen.push(data) },
    );
    expect(result).toEqual({ exitCode: 0 });
    // Pi's own stream is unchanged: the inner onData still saw every chunk.
    expect(innerSeen.map((b) => b.toString("utf8"))).toEqual(["\x1b]2;t\x07", "plain\n"]);
    expect(tap.seen.map((b) => b.toString("utf8"))).toEqual(["\x1b]2;t\x07", "plain\n"]);
  });

  test("the tapped bash tool delivers the raw OSC to the channel fd", async () => {
    const channel = openChannel("title-tap-");
    const definition = tappedBashTool(channel.dir, {
      getShellPath: () => undefined,
      getShellCommandPrefix: () => undefined,
    });
    const result: any = await definition.execute(
      "t1",
      { command: "printf '\\033]2;from the tool\\a'; echo done" },
      new AbortController().signal,
      undefined,
      undefined,
    );
    expect(result.exitCode ?? 0).toBe(0);
    // The channel is a full raw mirror (the host's emulator is the
    // interpreter and ignores plain text): OSC and plain text both land.
    const raw = readFileSync(channel.file, "utf8");
    expect(raw).toContain("\x1b]2;from the tool\x07");
    expect(raw).toContain("done\n");
    // The tool's own result is untouched: stripped text, no escapes.
    expect(JSON.stringify(result.content ?? result)).toContain("done");
    expect(raw.includes("\x1b[0m")).toBe(false);
    channel.restore();
  });

  test("a non-zero exit passes through and the tap still fed the channel", async () => {
    const channel = openChannel("title-tap-exit-");
    const definition = tappedBashTool(channel.dir, {
      getShellPath: () => undefined,
      getShellCommandPrefix: () => undefined,
    });
    const result: any = await definition.execute(
      "t-exit",
      { command: "printf '\\033]2;failing\\a'; exit 3" },
      new AbortController().signal,
      undefined,
      undefined,
    );
    // The exit code rides `structuredContent` (pi's BashToolOutput).
    expect(result.structuredContent.exit_code).toBe(3);
    expect(readFileSync(channel.file, "utf8")).toContain("\x1b]2;failing\x07");
    channel.restore();
  });

  test("the definition keeps pi's renderer and the user's shell settings", () => {
    const dir = mkdtempSync(join(tmpdir(), "title-tap-fidelity-"));
    dirs.push(dir);
    const definition = tappedBashTool(dir, {
      getShellPath: () => "/bin/bash",
      getShellCommandPrefix: () => "export ZU_PREFIX_MARK=1",
    });
    // createBashToolDefinition spreads the shell renderers in — a wrapper
    // that dropped them would break the tool card.
    expect(typeof definition.renderCall).toBe("function");
    expect(typeof definition.renderResult).toBe("function");
    // The user's commandPrefix reaches execution: the mark proves it ran.
    const promise = definition.execute(
      "t-prefix",
      { command: "echo $ZU_PREFIX_MARK" },
      new AbortController().signal,
      undefined,
      undefined,
    );
    return promise.then((result: any) => {
      expect(result.structuredContent?.output).toBe("1\n");
    });
  });

  test("two parallel bashes with split titles both reach the channel", async () => {
    const channel = openChannel("title-tap-par-");
    const definition = tappedBashTool(channel.dir, {
      getShellPath: () => undefined,
      getShellCommandPrefix: () => undefined,
    });
    // Deterministic split: each command prints half a title, sleeps past
    // the other's first half, then the rest — the channel must contain
    // both complete titles (the emulator reassembles per stream; a torn
    // title is the documented, accepted loss).
    const run = (half: string) =>
      definition.execute(
        "t-par",
        {
          command: `printf '\\033]2;${half}'; sleep 0.3; printf '${half}\\a'; echo ${half}-done`,
        },
        new AbortController().signal,
        undefined,
        undefined,
      );
    await Promise.all([run("alpha"), run("beta")]);
    const raw = readFileSync(channel.file, "utf8");
    expect(raw).toContain("\x1b]2;alpha");
    expect(raw).toContain("alpha\x07");
    expect(raw).toContain("\x1b]2;beta");
    expect(raw).toContain("beta\x07");
    expect(raw).toContain("alpha-done");
    expect(raw).toContain("beta-done");
    channel.restore();
  });

  test("100 MB through the tap stays fast and complete", async () => {
    const tap = collector();
    const inner: BashOperations = {
      exec: async (_command, _cwd, options) => {
        const chunk = Buffer.alloc(1024 * 1024, 0x61);
        for (let i = 0; i < 100; i++) options.onData(chunk);
        options.onData(Buffer.from("\x1b]2;after-100mb\x07"));
        return { exitCode: 0 };
      },
    };
    const started = Date.now();
    await tapBashOperations(inner, (data) => tap.write(data)).exec("x", "/tmp", {
      onData: () => {},
    });
    const elapsed = Date.now() - started;
    const total = tap.seen.reduce((sum, b) => sum + b.length, 0);
    expect(total).toBe(100 * 1024 * 1024 + 16);
    // The trailing title must survive the flood.
    expect(tap.seen[tap.seen.length - 1].toString("utf8")).toContain("after-100mb");
    expect(elapsed).toBeLessThan(10_000);
  });
});
