/**
 * Same-name tool precedence, on a real loader and a real session — the one
 * thing B3 had to verify against pi 1.5.0 instead of assuming.
 *
 * Conclusion, locked by these tests and by the comment in
 * `extensions/registry.ts`:
 *
 *   builtins win over user extensions (first registration wins, and
 *   `createPiServices` loads host inline extensions before the user's).
 *
 * The probe that established it: two inline factories registering `probeTool`
 * resolve to the earlier one (first registration wins), but the resource
 * loader appends every inline factory after the extension files from `~/.pi`,
 * so the file extension would win without the reorder.
 */

import { afterEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  createAgentSessionFromServices,
  SessionManager,
  type ExtensionAPI,
  type InlineExtension,
} from "@earendil-works/pi-coding-agent";
import { createPiServices } from "../pi/services";

const roots: string[] = [];

afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

const PROBE = "probeTool";

/** An extension factory that registers one tool whose description names its side. */
function probeFactory(side: string): (pi: ExtensionAPI) => void {
  return (pi) => {
    pi.registerTool({
      name: PROBE,
      description: `probe from ${side}`,
      parameters: { type: "object", properties: {}, additionalProperties: false },
      async execute() {
        return { content: [{ type: "text", text: side }], details: {} };
      },
    });
  };
}

/** A `~/.pi` package whose extension registers the same tool name. */
function userPackageWithProbe(root: string): string {
  const pkg = join(root, "user-pkg");
  mkdirSync(join(pkg, "extensions"), { recursive: true });
  writeFileSync(join(pkg, "package.json"), JSON.stringify({ name: "user-pkg", version: "0.0.0", keywords: ["pi-package"] }));
  writeFileSync(
    join(pkg, "extensions", "user.js"),
    "export default function (pi) { pi.registerTool({ name: " +
      JSON.stringify(PROBE) +
      ", description: \"probe from USER-PKG\", parameters: { type: \"object\", properties: {}, additionalProperties: false }, async execute() { return { content: [{ type: \"text\", text: \"user\" }] }; } }); }\n",
  );
  return pkg;
}

/** A temp agent dir (packages, no real `~/.pi`) and a temp cwd. */
function sandbox(): { root: string; agentDir: string; cwd: string } {
  const root = mkdtempSync(join(tmpdir(), "zu-precedence-"));
  roots.push(root);
  const agentDir = join(root, "agent");
  const cwd = join(root, "project");
  mkdirSync(agentDir, { recursive: true });
  mkdirSync(cwd, { recursive: true });
  return { root, agentDir, cwd };
}

async function probeSeenBySession(factories: InlineExtension[], agentDir: string, cwd: string) {
  const built = await createPiServices({ cwd, agentDir, trusted: true, extensionFactories: factories });
  const created = await createAgentSessionFromServices({
    services: built.services,
    sessionManager: SessionManager.inMemory(),
  });
  const probe = created.session.getAllTools().find((tool) => tool.name === PROBE);
  const order = built.services.resourceLoader.getExtensions().extensions.map((extension) => extension.path);
  created.session.dispose();
  return { probe, order };
}

describe("same-name tools", () => {
  test("builtins win over user extensions (first registration wins, host inline loads first)", async () => {
    const { root, agentDir, cwd } = sandbox();
    const pkg = userPackageWithProbe(root);
    writeFileSync(join(agentDir, "settings.json"), JSON.stringify({ packages: [pkg] }));

    const { probe, order } = await probeSeenBySession(
      [{ name: "zu:probe", factory: probeFactory("BUILTIN") }],
      agentDir,
      cwd,
    );

    expect(probe?.description).toBe("probe from BUILTIN");
    expect(probe?.sourceInfo?.path).toBe("<inline:zu:probe>");
    // The order is the mechanism: the host's inline extension precedes the package.
    expect(order.indexOf("<inline:zu:probe>")).toBeLessThan(order.findIndex((path) => path.includes("user-pkg")));
  });

  test("among host factories the earlier registration wins, so registry order is precedence", async () => {
    const { agentDir, cwd } = sandbox();
    const { probe } = await probeSeenBySession(
      [
        { name: "zu:first", factory: probeFactory("FIRST") },
        { name: "zu:second", factory: probeFactory("SECOND") },
      ],
      agentDir,
      cwd,
    );
    expect(probe?.description).toBe("probe from FIRST");
  });
});
