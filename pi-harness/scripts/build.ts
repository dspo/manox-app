/**
 * Builds the shipped `pi-acp` binary and the files that must sit beside it.
 *
 * `bun build --compile` produces a self-contained executable, but the runtime
 * reads three companions from the executable's own directory: the photon and
 * QuickJS wasm modules and the built-in extension skills. The packaging
 * scripts copy the whole `dist/` tree, so every artifact is one directory.
 *
 * The compile target follows the host (the release pipeline builds each
 * platform on its own runner); `PI_ACP_TARGET` overrides it for cross builds.
 */

import { copyFileSync, existsSync, mkdirSync, readdirSync, rmSync, statSync } from "node:fs";
import { join } from "node:path";

const ROOT = join(import.meta.dir, "..");
const DIST = join(ROOT, "dist");

/** Bun's cross-compilation target name for this host. */
function hostTarget(): string {
  const arch = process.arch === "arm64" ? "arm64" : "x64";
  if (process.platform === "darwin") return `bun-darwin-${arch}`;
  if (process.platform === "linux") return `bun-linux-${arch}`;
  throw new Error(`pi-acp has no bun target for ${process.platform}/${process.arch}`);
}

const target = process.env.PI_ACP_TARGET?.trim() || hostTarget();

/** The wasm modules the compiled binary loads from its own directory. */
const WASM: Array<{ from: string; name: string }> = [
  { from: "node_modules/@silvia-odwyer/photon-node/photon_rs_bg.wasm", name: "photon_rs_bg.wasm" },
  { from: "node_modules/quickjs-wasi/quickjs.wasm", name: "quickjs.wasm" },
];

function copyFile(from: string, to: string): void {
  const source = join(ROOT, from);
  if (!existsSync(source)) throw new Error(`missing ${from} — run \`bun install\` first`);
  copyFileSync(source, join(DIST, to));
}

/** Recursively copies `skills/` and collects the SKILL.md files it carried. */
function copySkills(from: string, to: string, landed: string[]): void {
  const source = join(ROOT, from);
  if (!existsSync(source)) return;
  mkdirSync(to, { recursive: true });
  for (const entry of readdirSync(source)) {
    const sourcePath = join(source, entry);
    const targetPath = join(to, entry);
    if (statSync(sourcePath).isDirectory()) copySkills(join(from, entry), targetPath, landed);
    else {
      copyFileSync(sourcePath, targetPath);
      if (entry === "SKILL.md") landed.push(targetPath);
    }
  }
}

/** Every SKILL.md in the source tree, by relative path. */
function skillManifests(from: string): string[] {
  const manifests: string[] = [];
  const walk = (dir: string): void => {
    for (const entry of readdirSync(dir)) {
      const path = join(dir, entry);
      if (statSync(path).isDirectory()) walk(path);
      else if (entry === "SKILL.md") manifests.push(path.slice(from.length + 1));
    }
  };
  const source = join(ROOT, from);
  if (existsSync(source)) walk(source);
  return manifests;
}

rmSync(DIST, { recursive: true, force: true });
mkdirSync(DIST, { recursive: true });

console.log(`pi-acp: compiling for ${target}`);
const compiled = Bun.spawnSync(
  [
    process.execPath,
    "build",
    "--compile",
    `--target=${target}`,
    "src/acp-main.ts",
    "--outfile",
    join(DIST, "pi-acp"),
  ],
  { cwd: ROOT, stdout: "inherit", stderr: "inherit" },
);
if (compiled.exitCode !== 0) throw new Error(`bun build --compile failed (exit ${compiled.exitCode})`);

for (const module of WASM) copyFile(module.from, module.name);

const landed: string[] = [];
copySkills("skills", join(DIST, "skills"), landed);
landed.sort();
const expected = skillManifests("skills").sort();
if (landed.length !== expected.length) {
  throw new Error(
    `skills did not all land beside pi-acp: expected [${expected.join(", ")}], got [${landed.join(", ")}]`,
  );
}

console.log(`pi-acp: dist/ built (${expected.length} skill manifest(s) beside the binary)`);
