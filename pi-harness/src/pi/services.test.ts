/**
 * Runtime parity tests that do not need a model: the official built-ins
 * register under the same loader semantics as the CLI, packages and skills
 * are discovered, trust follows the CLI precedence (with an elicitation
 * question at the ask step), and ACP MCP servers map onto pi's config.
 */

import { afterEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DefaultResourceLoader, SettingsManager } from "@earendil-works/pi-coding-agent";
import { acpMcpExtension, toPiMcpConfig } from "./mcp";
import { OFFICIAL_BUILTIN_EXTENSIONS } from "./official";
import { createPiServices } from "./services";
import { resolveProjectTrust } from "./trust";

const roots: string[] = [];

function sandbox(): { root: string; agentDir: string; cwd: string } {
  const root = mkdtempSync(join(tmpdir(), "zu-pi-parity-"));
  roots.push(root);
  const agentDir = join(root, "agent");
  const cwd = join(root, "project");
  mkdirSync(agentDir, { recursive: true });
  mkdirSync(cwd, { recursive: true });
  return { root, agentDir, cwd };
}

afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

async function extensionsOf(options: {
  cwd: string;
  agentDir: string;
  settings?: Record<string, unknown>;
}): Promise<string[]> {
  if (options.settings) {
    writeFileSync(join(options.agentDir, "settings.json"), JSON.stringify(options.settings));
  }
  const settings = SettingsManager.create(options.cwd, options.agentDir, { projectTrusted: true });
  const loader = new DefaultResourceLoader({
    cwd: options.cwd,
    agentDir: options.agentDir,
    settingsManager: settings,
    extensionFactories: OFFICIAL_BUILTIN_EXTENSIONS,
  });
  await loader.reload();
  return loader.getExtensions().extensions.map((extension) => extension.path);
}

describe("official built-ins under the loader", () => {
  test("codemode, tool-search and mcp register as builtin resources", async () => {
    const { cwd, agentDir } = sandbox();
    const paths = await extensionsOf({ cwd, agentDir });
    expect(paths).toContain("builtin:codemode");
    expect(paths).toContain("builtin:tool-search");
    expect(paths).toContain("builtin:mcp");
  });

  test("createPiServices assembles the same builtins through the official services path", async () => {
    const { cwd, agentDir } = sandbox();
    const built = await createPiServices({ cwd, agentDir, trusted: true });
    const paths = built.services.resourceLoader.getExtensions().extensions.map((extension) => extension.path);
    expect(paths).toContain("builtin:codemode");
    expect(paths).toContain("builtin:tool-search");
    expect(paths).toContain("builtin:mcp");
    expect(built.trusted).toBe(true);
  });

  test("a -builtin setting removes one exactly like the CLI", async () => {
    const { cwd, agentDir } = sandbox();
    const paths = await extensionsOf({ cwd, agentDir, settings: { extensions: ["-builtin:mcp"] } });
    expect(paths).toContain("builtin:codemode");
    expect(paths).not.toContain("builtin:mcp");
  });
});

describe("official packages and skills", () => {
  test("a local package's extension and skill are discovered", async () => {
    const { root, cwd, agentDir } = sandbox();
    const pkg = join(root, "fixture-pkg");
    mkdirSync(join(pkg, "extensions"), { recursive: true });
    mkdirSync(join(pkg, "skills", "demo"), { recursive: true });
    writeFileSync(
      join(pkg, "package.json"),
      JSON.stringify({ name: "fixture-pkg", version: "0.0.0", keywords: ["pi-package"] }),
    );
    writeFileSync(
      join(pkg, "extensions", "fixture.js"),
      "export default function (pi) { pi.registerCommand('fixture', { description: 'fixture', handler: async () => {} }); }\n",
    );
    writeFileSync(
      join(pkg, "skills", "demo", "SKILL.md"),
      "---\nname: demo\ndescription: A fixture skill\n---\n\nDo the fixture.\n",
    );

    writeFileSync(join(agentDir, "settings.json"), JSON.stringify({ packages: [pkg] }));
    const settings = SettingsManager.create(cwd, agentDir, { projectTrusted: true });
    const loader = new DefaultResourceLoader({
      cwd,
      agentDir,
      settingsManager: settings,
      extensionFactories: [],
    });
    await loader.reload();
    const paths = loader.getExtensions().extensions.map((extension) => extension.path);
    expect(paths.some((path) => path.includes("fixture-pkg") && path.includes("fixture.js"))).toBe(true);
    expect(loader.getSkills().skills.some((skill) => skill.name === "demo")).toBe(true);
  });
});

describe("project trust", () => {
  test("no trust-requiring resources means trusted", async () => {
    const { cwd, agentDir } = sandbox();
    const decision = await resolveProjectTrust({ cwd, agentDir });
    expect(decision).toMatchObject({ required: false, trusted: true, source: "no-resources" });
  });

  test("ask without a client is untrusted; an accepted elicitation is remembered", async () => {
    const { cwd, agentDir } = sandbox();
    mkdirSync(join(cwd, ".pi"), { recursive: true });
    writeFileSync(join(cwd, ".pi", "settings.json"), "{}\n");
    const headless = await resolveProjectTrust({ cwd, agentDir });
    expect(headless).toMatchObject({ required: true, trusted: false, source: "no-ui" });

    const accepted = await resolveProjectTrust({
      cwd,
      agentDir,
      elicit: async () => ({ action: "accept", content: { trusted: true } }),
    });
    expect(accepted).toMatchObject({ required: true, trusted: true, source: "elicitation" });

    const remembered = await resolveProjectTrust({ cwd, agentDir });
    expect(remembered).toMatchObject({ required: true, trusted: true, source: "store" });
  });

  test("declining is session-only", async () => {
    const { cwd, agentDir } = sandbox();
    mkdirSync(join(cwd, ".pi"), { recursive: true });
    writeFileSync(join(cwd, ".pi", "settings.json"), "{}\n");
    const declined = await resolveProjectTrust({
      cwd,
      agentDir,
      elicit: async () => ({ action: "decline" }),
    });
    expect(declined).toMatchObject({ required: true, trusted: false, source: "elicitation" });
    const again = await resolveProjectTrust({ cwd, agentDir });
    expect(again).toMatchObject({ required: true, trusted: false, source: "no-ui" });
  });

  test("an extension project_trust handler decides before the store", async () => {
    const { cwd, agentDir } = sandbox();
    mkdirSync(join(cwd, ".pi"), { recursive: true });
    writeFileSync(join(cwd, ".pi", "settings.json"), "{}\n");
    const settings = SettingsManager.create(cwd, agentDir, { projectTrusted: false });
    const loader = new DefaultResourceLoader({
      cwd,
      agentDir,
      settingsManager: settings,
      extensionFactories: [
        (pi) => {
          pi.on("project_trust", () => ({ trusted: "yes", remember: true }));
        },
      ],
    });
    await loader.reload();
    const decision = await resolveProjectTrust({
      cwd,
      agentDir,
      extensionsResult: loader.getExtensions(),
    });
    expect(decision).toMatchObject({ required: true, trusted: true, source: "extension" });
    expect((await resolveProjectTrust({ cwd, agentDir })).source).toBe("store");
  });

  test("an undecided handler falls through to the ACP question", async () => {
    const { cwd, agentDir } = sandbox();
    mkdirSync(join(cwd, ".pi"), { recursive: true });
    writeFileSync(join(cwd, ".pi", "settings.json"), "{}\n");
    const settings = SettingsManager.create(cwd, agentDir, { projectTrusted: false });
    const loader = new DefaultResourceLoader({
      cwd,
      agentDir,
      settingsManager: settings,
      extensionFactories: [
        (pi) => {
          pi.on("project_trust", () => ({ trusted: "undecided" }));
        },
      ],
    });
    await loader.reload();
    const accepted = await resolveProjectTrust({
      cwd,
      agentDir,
      extensionsResult: loader.getExtensions(),
      elicit: async () => ({ action: "accept", content: { trusted: true } }),
    });
    expect(accepted).toMatchObject({ required: true, trusted: true, source: "elicitation" });
  });

  test("defaultProjectTrust=always wins without a question", async () => {
    const { cwd, agentDir } = sandbox();
    mkdirSync(join(cwd, ".pi"), { recursive: true });
    writeFileSync(join(cwd, ".pi", "settings.json"), "{}\n");
    writeFileSync(join(agentDir, "settings.json"), JSON.stringify({ defaultProjectTrust: "always" }));
    const decision = await resolveProjectTrust({ cwd, agentDir });
    expect(decision).toMatchObject({ required: true, trusted: true, source: "default" });
  });
});

describe("ACP MCP servers", () => {
  test("stdio env arrays become records, and servers register directly", () => {
    const config = toPiMcpConfig({
      name: "local",
      command: "/bin/echo",
      args: ["hi"],
      env: [{ name: "A", value: "1" }],
      cwd: "/tmp",
    });
    expect(config).toMatchObject({
      type: "stdio",
      command: "/bin/echo",
      args: ["hi"],
      env: { A: "1" },
      cwd: "/tmp",
      exposure: "direct",
    });
  });

  test("http passes through; sse is connected as http", () => {
    const config = toPiMcpConfig({ name: "web", type: "sse", url: "https://example.test/mcp", headers: { A: "1" } });
    expect(config).toMatchObject({ type: "http", url: "https://example.test/mcp", headers: { A: "1" }, exposure: "direct" });
  });

  test("the extension registers the client's servers by name", () => {
    const registered: Array<[string, unknown]> = [];
    const extension = acpMcpExtension([{ name: "local", command: "/bin/echo" }]);
    if (typeof extension === "function") throw new Error("expected a named extension");
    extension.factory({ registerMcpServer: (name: string, config: unknown) => registered.push([name, config]) } as never);
    expect(registered.map(([name]) => name)).toEqual(["local"]);
  });
});
