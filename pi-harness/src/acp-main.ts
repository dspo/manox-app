/**
 * pi-harness distro, ACP entry: the agent side of ACP v1 over stdio ndjson
 * JSON-RPC 2.0 (docs/pi-acp-PROTOCOL.md §4/§5).
 *
 * stdout carries the protocol and nothing else — every log line goes to
 * stderr. The process serves one connection; the client closing stdin is the
 * graceful exit (flush and dispose), signals are the fallback. The legacy
 */

import { ndJsonStream } from "@agentclientprotocol/sdk";
import { createAcpApp } from "./acp/app";
import { createPiPorts } from "./acp/pi-session";

const log = (line: string): void => {
  console.error(`[pi-acp] ${line}`);
};

const app = createAcpApp(createPiPorts(log), log);

const stdout = new WritableStream<Uint8Array>({
  write(chunk) {
    process.stdout.write(chunk);
  },
});

const connection = app.connect(ndJsonStream(stdout, Bun.stdin.stream()));

/** Settle-and-exit: dispose pi sessions, then leave with a clean status. */
function shutdown(code: number): void {
  log("shutting down");
  try {
    connection.close();
  } catch {
    // Already closed.
  }
  process.exit(code);
}

// The client closing stdin is ACP's graceful exit; the SDK flushes and the
// connection closes. Disposing after that releases pi's sessions.
void connection.closed.then(() => {
  log("connection closed; exiting");
  process.exit(0);
});

for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, () => shutdown(0));
}

process.on("uncaughtException", (error) => {
  log(`uncaught exception: ${error instanceof Error ? (error.stack ?? error.message) : String(error)}`);
  shutdown(1);
});
