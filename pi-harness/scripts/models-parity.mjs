/**
 * Compares a session's model list with the CLI's, on this machine.
 *
 * P0-1's acceptance: a new session lists exactly the models
 * `pi --list-models` prints. The engine is started the way the app starts it
 * (its own source, a temporary state directory) and asked the same question
 * the composer's picker asks (`session.models`); `pi` is run beside it. A
 * difference is printed as `provider/model` lines and exits non-zero.
 *
 *   cd engine && bun run models:parity
 */

import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const stateDir = mkdtempSync(join(tmpdir(), "zu-parity-"));
const token = "parity";
const port = 47000 + Math.floor(Math.random() * 1000);

const engine = spawn(process.execPath, ["run", "src/server.ts"], {
  cwd: import.meta.dir + "/..",
  env: { ...process.env, ZU_PORT: String(port), ZU_TOKEN: token, ZU_STATE_DIR: stateDir },
  stdio: ["ignore", "pipe", "pipe"],
});
let output = "";
engine.stdout.on("data", (chunk) => {
  output += String(chunk);
});
engine.stderr.on("data", (chunk) => {
  process.stderr.write(chunk);
});

/** The CLI's list: `provider model …` rows after the header. */
function cliModels() {
  const text = execFileSync("pi", ["--list-models"], { encoding: "utf8" });
  return new Set(
    text
      .split("\n")
      .slice(1)
      .map((line) => line.trim().split(/\s+/))
      .filter((row) => row.length >= 2)
      .map(([provider, model]) => `${provider}/${model}`),
  );
}

/** The engine's answer over the wire, asked like the picker asks. */
function engineModels() {
  const socket = new WebSocket(`ws://127.0.0.1:${port}`);
  const waiting = new Map();
  const send = (message) => socket.send(JSON.stringify({ v: 1, ...message }));
  const ask = (type, payload, id) =>
    new Promise((resolve) => {
      waiting.set(id, resolve);
      send({ id, type, payload });
    });
  socket.onmessage = (event) => {
    const message = JSON.parse(String(event.data));
    const resolve = waiting.get(message.id);
    if (!resolve) return;
    waiting.delete(message.id);
    resolve(message.payload);
  };
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("the engine did not answer")), 120000);
    socket.onerror = () => reject(new Error(`the engine refused port ${port}`));
    socket.onopen = async () => {
      send({ type: "hello", token });
      await new Promise((done) => setTimeout(done, 300));
      const created = await ask("session.create", { cwd: tmpdir() }, "1");
      const reply = await ask("session.models", { sessionId: created.data.id }, "2");
      await ask("session.close", { sessionId: created.data.id }, "3");
      clearTimeout(timer);
      socket.close();
      resolve(reply.data);
    };
  });
}

async function main() {
  for (let waited = 0; waited < 30000 && !output.includes("listening"); waited += 50) {
    await new Promise((done) => setTimeout(done, 50));
  }
  if (!output.includes("listening")) throw new Error("the engine did not start");
  const models = await engineModels();
  const cli = cliModels();
  const listed = new Set(
    (models.providers ?? []).flatMap((provider) =>
      provider.models.map((model) => `${provider.id}/${model.id}`),
    ),
  );
  const missing = [...cli].filter((row) => !listed.has(row)).sort();
  const extra = [...listed].filter((row) => !cli.has(row)).sort();
  console.log(`pi --list-models: ${cli.size}  session.models: ${listed.size}`);
  if (models.reason) console.log(`reason: ${JSON.stringify(models.reason)}`);
  for (const row of missing) console.log(`  missing from session.models: ${row}`);
  for (const row of extra) console.log(`  not in pi --list-models: ${row}`);
  if (cli.size === 0) {
    console.log("pi lists nothing on this machine; nothing to compare");
  } else if (missing.length === 0 && extra.length === 0) {
    console.log("the two lists are equal");
  }
  return missing.length + extra.length === 0 && cli.size > 0 ? 0 : 1;
}

main()
  .then((code) => {
    engine.kill("SIGTERM");
    rmSync(stateDir, { recursive: true, force: true });
    process.exit(code);
  })
  .catch((error) => {
    console.error(String(error?.message ?? error));
    engine.kill("SIGTERM");
    rmSync(stateDir, { recursive: true, force: true });
    process.exit(1);
  });
