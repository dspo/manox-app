import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { convertToLlm, SessionManager } from "@earendil-works/pi-coding-agent";
import {
  createPiSubagentRunner,
  deriveSubagentSessionFile,
  discoverAgents,
  formatCompletionBatch,
  SubagentManager,
  type CaptainLike,
  type SubagentHost,
  type SubagentRun,
  type SubagentRunner,
  type SubagentRunnerInput,
} from "./subagents";

function workspace(): string {
  return mkdtempSync(join(tmpdir(), "zu-subagents-"));
}

function captain(root: string, id = "cap-1"): CaptainLike {
  return {
    id,
    path: join(root, "captain.jsonl"),
    cwd: root,
    containerId: "c1",
    manager: { getSessionFile: () => join(root, "captain.jsonl") } as any,
    session: { model: undefined, thinkingLevel: undefined },
  };
}

/** A host over an in-memory settled index; notes/emits are recorded. */
function testHost(root: string, overrides: Partial<SubagentHost> = {}): SubagentHost {
  const settled: SubagentRun[] = [];
  return {
    agentDir: join(root, "agent"),
    log: () => {},
    captainFor: (id) => (id === "cap-1" ? captain(root, id) : undefined),
    settledRuns: () => settled,
    saveSettled: (run) => {
      const index = settled.findIndex((entry) => entry.runId === run.runId);
      if (index >= 0) settled[index] = { ...run };
      else settled.push({ ...run });
    },
    appendCaptainNote: () => {},
    emit: () => {},
    projectTrusted: () => true,
    isArchived: () => false,
    ...overrides,
  };
}

/** A runner that holds every run until the test releases it. */
function holdRunner(): { runner: SubagentRunner; calls: SubagentRunnerInput[]; releases: Array<() => void> } {
  const calls: SubagentRunnerInput[] = [];
  const releases: Array<() => void> = [];
  const runner: SubagentRunner = {
    async run(input) {
      calls.push(input);
      await new Promise<void>((resolve) => releases.push(resolve));
      return { output: `done ${input.task}`, aborted: false, sessionPath: join(tmpdir(), `${input.run.runId}.jsonl`) };
    },
  };
  return { runner, calls, releases };
}

function writeAgent(dir: string, file: string, frontmatter: string, body: string): void {
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, file), `---\n${frontmatter}---\n${body}\n`);
}

describe("agent definitions", () => {
  test("builtin wins over personal and project; project adds names but cannot replace", () => {
    const root = workspace();
    const agentDir = join(root, "agent");
    const cwd = join(root, "repo");
    writeAgent(join(agentDir, "agents"), "worker.md", "name: worker\n", "personal worker");
    writeAgent(join(agentDir, "agents"), "helper.md", "name: helper\ntools: [read]\n", "personal helper");
    writeAgent(join(cwd, ".pi", "agents"), "worker.md", "name: worker\ntools: [bash]\n", "project worker");
    writeAgent(join(cwd, ".pi", "agents"), "helper.md", "name: helper\ntools: [bash]\n", "project helper");
    writeAgent(join(cwd, ".agents", "agents"), "extra.md", "name: extra\nmax_turns: 3\n", "project extra");

    const ignored: string[] = [];
    const agents = discoverAgents(cwd, agentDir, true, (line) => ignored.push(line));
    const worker = agents.find((agent) => agent.name === "worker");
    expect(worker?.source).toBe("builtin");
    expect(worker?.tools).toBeUndefined();
    // Personal comes before project: first definition wins for the same name.
    const helper = agents.find((agent) => agent.name === "helper");
    expect(helper?.source).toBe("personal");
    expect(helper?.systemPrompt).toBe("personal helper");
    const extra = agents.find((agent) => agent.name === "extra");
    expect(extra?.source).toBe("project");
    expect(extra?.maxTurns).toBe(3);
    expect(ignored.some((line) => line.includes("worker") && line.includes("builtin wins"))).toBe(true);
  });

  test("an untrusted project loads no project definitions", () => {
    const root = workspace();
    const agentDir = join(root, "agent");
    const cwd = join(root, "repo");
    writeAgent(join(cwd, ".agents", "agents"), "extra.md", "name: extra\n", "project extra");
    const agents = discoverAgents(cwd, agentDir, false);
    expect(agents.some((agent) => agent.name === "extra")).toBe(false);
  });

  test("a malformed max_turns is dropped, not fatal", () => {
    const root = workspace();
    const agentDir = join(root, "agent");
    writeAgent(join(agentDir, "agents"), "bad.md", "name: bad\nmax_turns: 0\n", "bad");
    writeAgent(join(agentDir, "agents"), "worse.md", "name: worse\nmax_turns: many\n", "worse");
    const agents = discoverAgents(root, agentDir, false);
    expect(agents.find((agent) => agent.name === "bad")?.maxTurns).toBeUndefined();
    expect(agents.find((agent) => agent.name === "worse")?.maxTurns).toBeUndefined();
  });
});

describe("scheduling", () => {
  test("five dispatches run four and queue the fifth; a settle starts it", async () => {
    const root = workspace();
    const { runner, calls, releases } = holdRunner();
    const manager = new SubagentManager(testHost(root), runner);
    const runs = Array.from({ length: 5 }, (_, index) => manager.spawn("cap-1", "scout", `task ${index}`));

    const live = manager.list({ captainId: "cap-1" });
    expect(live.filter((run) => run.status === "running")).toHaveLength(4);
    expect(live.filter((run) => run.status === "queued")).toHaveLength(1);
    expect(calls).toHaveLength(4);

    releases[0]();
    await manager.wait(runs[0].runId);
    await Bun.sleep(1);
    expect(manager.run(runs[4].runId)?.status).toBe("running");
    expect(calls).toHaveLength(5);
    for (const release of releases.slice(1)) release();
    await Promise.all(runs.map((run) => manager.wait(run.runId)));
  });

  test("aborting a queued run settles it without ever calling the runner", async () => {
    const root = workspace();
    const { runner, calls, releases } = holdRunner();
    const manager = new SubagentManager(testHost(root), runner);
    const runs = Array.from({ length: 5 }, (_, index) => manager.spawn("cap-1", "scout", `task ${index}`));

    await manager.abort(runs[4].runId);
    const queued = await manager.wait(runs[4].runId);
    expect(queued?.status).toBe("aborted");
    expect(queued?.output).toBe("cancelled before start");
    expect(calls).toHaveLength(4);

    for (const release of releases) release();
    await Promise.all(runs.slice(0, 4).map((run) => manager.wait(run.runId)));
  });

  test("aborting a running run reaches the child session it attached", async () => {
    const root = workspace();
    let release: () => void = () => {};
    const runner: SubagentRunner = {
      async run(input) {
        input.attach({
          abort: async () => {
            release();
          },
        });
        await new Promise<void>((resolve) => {
          release = resolve;
        });
        return { output: "stopped", aborted: true, sessionPath: join(root, "run.jsonl") };
      },
    };
    const manager = new SubagentManager(testHost(root), runner);
    const run = manager.spawn("cap-1", "scout", "stop me");
    await manager.abort(run.runId);
    const settled = await manager.wait(run.runId);
    expect(settled?.status).toBe("aborted");
  });

  test("archiving cancels queued runs without starting another", async () => {
    const root = workspace();
    const { runner, calls, releases } = holdRunner();
    const manager = new SubagentManager(testHost(root), runner);
    const runs = Array.from({ length: 5 }, (_, index) => manager.spawn("cap-1", "scout", `task ${index}`));

    await manager.archiveCaptain("cap-1");
    expect(calls).toHaveLength(4);
    expect(manager.run(runs[4].runId)?.status).toBe("interrupted");
    expect(manager.run(runs[0].runId)?.status).toBe("interrupted");
    for (const release of releases) release();
    await Promise.all(runs.map((run) => manager.wait(run.runId)));
  });
});

describe("turn cap", () => {
  test("a max_turns run is steered once at the cap and aborted on the next turn", async () => {
    const root = workspace();
    const agentDir = join(root, "agent");
    writeAgent(join(agentDir, "agents"), "capped.md", "name: capped\nmax_turns: 3\n", "capped");
    const actions: string[] = [];
    const runner: SubagentRunner = {
      async run(input) {
        for (let turn = 0; turn < 4; turn++) actions.push(input.onTurn({ continuing: true }));
        return { output: "final", aborted: true, sessionPath: join(root, "run.jsonl") };
      },
    };
    const manager = new SubagentManager(testHost(root), runner);
    const run = manager.spawn("cap-1", "capped", "keep going");
    const settled = await manager.wait(run.runId);
    expect(actions).toEqual(["continue", "continue", "wrap-up", "abort"]);
    expect(settled?.status).toBe("aborted");
  });

  test("a run that finishes within the cap keeps its result", async () => {
    const root = workspace();
    const agentDir = join(root, "agent");
    writeAgent(join(agentDir, "agents"), "capped.md", "name: capped\nmax_turns: 3\n", "capped");
    const actions: string[] = [];
    const runner: SubagentRunner = {
      async run(input) {
        actions.push(input.onTurn({ continuing: false }));
        actions.push(input.onTurn({ continuing: false }));
        return { output: "verdict: pass", aborted: false, sessionPath: join(root, "run.jsonl") };
      },
    };
    const manager = new SubagentManager(testHost(root), runner);
    const run = manager.spawn("cap-1", "capped", "finish early");
    const settled = await manager.wait(run.runId);
    expect(actions).toEqual(["continue", "continue"]);
    expect(settled?.status).toBe("settled");
    expect(settled?.verdict).toBe("pass");
  });
});

describe("completion delivery", () => {
  function immediateRunner(output = "the result"): SubagentRunner {
    return {
      async run(input) {
        return { output, aborted: false, sessionPath: join(tmpdir(), `${input.run.runId}.jsonl`) };
      },
    };
  }

  test("a settle is pushed once at the next turn boundary and then claimed", async () => {
    const root = workspace();
    const manager = new SubagentManager(testHost(root), immediateRunner());
    const sent: Array<{ batch: SubagentRun[]; triggerTurn: boolean }> = [];
    manager.setDelivery({
      send: (batch, options) => {
        sent.push({ batch, triggerTurn: options.triggerTurn });
        return true;
      },
      isIdle: () => false,
    });

    const run = manager.spawn("cap-1", "scout", "find it");
    await manager.wait(run.runId);
    expect(sent).toHaveLength(0);

    manager.flushCompletions({ triggerTurn: false });
    expect(sent).toHaveLength(1);
    expect(sent[0].batch.map((entry) => entry.runId)).toEqual([run.runId]);
    expect(sent[0].batch[0].collected).toBe(true);
    expect(manager.run(run.runId)?.collected).toBe(true);

    // A second flush has nothing left: the completion is delivered once.
    manager.flushCompletions({ triggerTurn: false });
    expect(sent).toHaveLength(1);
  });

  test("list_agents claims before the push, so the result is not repeated", async () => {
    const root = workspace();
    const manager = new SubagentManager(testHost(root), immediateRunner());
    const sent: SubagentRun[][] = [];
    manager.setDelivery({
      send: (batch) => {
        sent.push(batch);
        return true;
      },
      isIdle: () => false,
    });
    const run = manager.spawn("cap-1", "scout", "find it");
    await manager.wait(run.runId);
    manager.claim([run.runId]);
    manager.flushCompletions({ triggerTurn: false });
    expect(sent).toHaveLength(0);
  });

  test("a rejected send restores the run to pending", async () => {
    const root = workspace();
    const manager = new SubagentManager(testHost(root), immediateRunner());
    let accept = false;
    const sent: SubagentRun[][] = [];
    manager.setDelivery({
      send: (batch) => {
        if (!accept) return false;
        sent.push(batch);
        return true;
      },
      isIdle: () => false,
    });
    const run = manager.spawn("cap-1", "scout", "find it");
    await manager.wait(run.runId);
    manager.flushCompletions({ triggerTurn: false });
    expect(sent).toHaveLength(0);
    expect(manager.run(run.runId)?.collected).toBe(false);

    accept = true;
    manager.flushCompletions({ triggerTurn: false });
    expect(sent).toHaveLength(1);
    expect(sent[0][0].runId).toBe(run.runId);
  });

  test("an idle captain is woken once, with triggerTurn", async () => {
    const root = workspace();
    const manager = new SubagentManager(testHost(root), immediateRunner());
    const sent: Array<{ triggerTurn: boolean }> = [];
    manager.setDelivery({
      send: (_batch, options) => {
        sent.push({ triggerTurn: options.triggerTurn });
        return true;
      },
      isIdle: () => true,
    });
    const run = manager.spawn("cap-1", "scout", "find it");
    await manager.wait(run.runId);
    await Bun.sleep(1);
    expect(sent).toEqual([{ triggerTurn: true }]);
  });

  test("a user-stopped run is recorded but not pushed", async () => {
    const root = workspace();
    const { runner, releases } = holdRunner();
    const manager = new SubagentManager(testHost(root), runner);
    const sent: SubagentRun[][] = [];
    manager.setDelivery({ send: (batch) => (sent.push(batch), true), isIdle: () => false });
    const run = manager.spawn("cap-1", "scout", "stop me");
    await manager.abort(run.runId);
    releases[0]();
    await manager.wait(run.runId);
    manager.flushCompletions({ triggerTurn: false });
    expect(manager.run(run.runId)?.status).toBe("aborted");
    expect(sent).toHaveLength(0);
  });

  test("the completion message becomes user content in the provider context", () => {
    // pi 1.5.0 semantics, pinned: `convertToLlm` maps a custom message to a
    // user message. This is why delivery can use sendMessage instead of a
    // queue that only the UI sees. A pi upgrade that changes this fails here.
    const batch: SubagentRun[] = [
      {
        runId: "run-1",
        captainId: "cap-1",
        containerId: "c1",
        agent: "scout",
        task: "find the flag",
        status: "settled",
        startedAt: 1,
        output: "the flag is here",
      },
    ];
    const text = formatCompletionBatch(batch);
    const message = {
      role: "custom",
      customType: "zu.subagent.completion",
      content: [{ type: "text", text }],
      display: true,
      details: { runs: ["run-1"] },
      timestamp: new Date().toISOString(),
    };
    const context = convertToLlm([message as any]);
    expect(context).toHaveLength(1);
    expect((context[0] as any).role).toBe("user");
    expect(JSON.stringify(context[0])).toContain("the flag is here");
  });
});

describe("lineage", () => {
  test("the state root follows ZU_STATE_DIR when the app injects it", () => {
    const root = workspace();
    const previousStateDir = process.env.ZU_STATE_DIR;
    const previousAgentDir = process.env.PI_CODING_AGENT_DIR;
    const captainDir = join(root, "steer", "subagents", "cap-1");
    mkdirSync(captainDir, { recursive: true });
    const sessionFile = join(captainDir, "123_run-abc.jsonl");
    writeFileSync(sessionFile, "");
    process.env.ZU_STATE_DIR = join(root, "steer");
    process.env.PI_CODING_AGENT_DIR = join(root, "agent");
    try {
      expect(deriveSubagentSessionFile("cap-1", "run-abc")).toBe(sessionFile);
    } finally {
      if (previousStateDir === undefined) delete process.env.ZU_STATE_DIR;
      else process.env.ZU_STATE_DIR = previousStateDir;
      if (previousAgentDir === undefined) delete process.env.PI_CODING_AGENT_DIR;
      else process.env.PI_CODING_AGENT_DIR = previousAgentDir;
    }
  });

  test("a lost index path is recovered from the run id suffix", () => {
    const root = workspace();
    const captainDir = join(root, "zu", "subagents", "cap-1");
    mkdirSync(captainDir, { recursive: true });
    writeFileSync(join(captainDir, "20260101T000000_run-abc.jsonl"), "");
    expect(deriveSubagentSessionFile("cap-1", "run-abc", { root: captainDir })).toBe(
      join(captainDir, "20260101T000000_run-abc.jsonl"),
    );
    expect(deriveSubagentSessionFile("cap-1", "run-missing", { root: captainDir })).toBeUndefined();
    expect(deriveSubagentSessionFile("cap-1", "run-abc", { sessionPath: "/index/path.jsonl", root: captainDir })).toBe(
      "/index/path.jsonl",
    );
  });

  test("a settled run with no session file is an explicit error, not an empty list", () => {
    const root = workspace();
    const host = testHost(root);
    const manager = new SubagentManager(host, {
      async run() {
        return { output: "", aborted: false, sessionPath: "" };
      },
    });
    host.saveSettled({
      runId: "run-lost",
      captainId: "cap-1",
      containerId: "c1",
      agent: "scout",
      task: "gone",
      status: "settled",
      startedAt: 1,
    });
    expect(() => manager.transcript("run-lost")).toThrow(/not recoverable/);
  });

  test("a queued run has an empty transcript, not a missing file", () => {
    const root = workspace();
    const { runner, releases } = holdRunner();
    const manager = new SubagentManager(testHost(root), runner);
    const runs = Array.from({ length: 5 }, (_, index) => manager.spawn("cap-1", "scout", `task ${index}`));
    expect(manager.transcript(runs[4].runId).messages).toEqual([]);
    for (const release of releases) release();
  });

  test("a settled run whose index lost its path reads from the run id suffix", () => {
    const root = workspace();
    const host = testHost(root);
    const dir = join(root, "agent", "..", "zu", "subagents", "cap-1");
    mkdirSync(dir, { recursive: true });
    SessionManager.create(root, dir, { id: "run-abc" })
      .appendMessage({ role: "user", content: [{ type: "text", text: "hello" }], timestamp: Date.now() } as any);
    host.saveSettled({
      runId: "run-abc",
      captainId: "cap-1",
      containerId: "c1",
      agent: "scout",
      task: "found",
      status: "settled",
      startedAt: 1,
    });
    const manager = new SubagentManager(host);
    const data = manager.transcript("run-abc");
    expect(data.messages.map((message) => message.text)).toEqual(["hello"]);
  });
});

describe("depth defence", () => {
  test("the real runner excludes dispatch and list_agents", async () => {
    const root = workspace();
    const captured: any[] = [];
    const runner = createPiSubagentRunner(
      { agentDir: join(root, "agent"), projectTrusted: () => true },
      {
        createServices: async () => ({ services: {} }),
        createSession: async (options) => {
          captured.push(options);
          return {
            session: {
              state: { messages: [] },
              prompt: async () => {},
              subscribe: () => () => {},
              dispose: () => {},
            },
          };
        },
      },
    );
    await runner.run({
      run: {
        runId: "run-1",
        captainId: "cap-1",
        containerId: "c1",
        agent: "worker",
        task: "t",
        status: "running",
        startedAt: 1,
      },
      captain: captain(root),
      definition: {
        name: "worker",
        description: "",
        systemPrompt: "you are a worker",
        source: "builtin",
      },
      task: "t",
      resume: false,
      onTurn: () => "continue",
      attach: () => {},
    });
    expect(captured[0].excludeTools).toEqual(["dispatch", "list_agents"]);
  });
});
