/**
 * The ACP `session/new` MCP servers, handed to pi's own MCP extension.
 *
 * `mcp.json` is still the official config and still loads through
 * `createMcpExtension()`. These servers are the client-supplied ones: ACP
 * expects the agent to connect what the client asked for, so they are
 * registered as `direct` (visible to the model) rather than pi's default
 * `codemode` exposure. A same-named `mcp.json` entry wins inside the MCP
 * extension, matching pi's normal precedence.
 */

import type { InlineExtension, McpServerConfig } from "@earendil-works/pi-coding-agent";

/** The ACP wire shape, deliberately loose: it comes from a peer. */
export interface AcpMcpServer {
  name?: string;
  type?: string;
  command?: string;
  args?: string[];
  env?: Array<{ name?: string; value?: string }> | Record<string, string>;
  cwd?: string;
  url?: string;
  headers?: Record<string, string>;
}

function envRecord(env: AcpMcpServer["env"]): Record<string, string> | undefined {
  if (!env) return undefined;
  if (!Array.isArray(env)) return env;
  const record: Record<string, string> = {};
  for (const entry of env) {
    if (typeof entry?.name === "string" && entry.name) record[entry.name] = String(entry.value ?? "");
  }
  return Object.keys(record).length > 0 ? record : undefined;
}

/** One ACP server as pi's MCP config. Throws when neither command nor url exists. */
export function toPiMcpConfig(server: AcpMcpServer): McpServerConfig {
  if (typeof server.command === "string" && server.command) {
    return {
      type: "stdio",
      command: server.command,
      ...(Array.isArray(server.args) && server.args.length > 0 ? { args: server.args } : {}),
      ...(envRecord(server.env) ? { env: envRecord(server.env) } : {}),
      ...(server.cwd ? { cwd: server.cwd } : {}),
      exposure: "direct",
    };
  }
  if (typeof server.url === "string" && server.url) {
    // ACP's `sse` transport is not a pi config type; its URL and headers are
    // the same shape as Streamable HTTP, so it is connected as `http`.
    return {
      type: "http",
      url: server.url,
      ...(server.headers && Object.keys(server.headers).length > 0 ? { headers: server.headers } : {}),
      exposure: "direct",
    };
  }
  throw new Error(`MCP server ${server.name ?? "(unnamed)"} has neither command nor url`);
}

/** A hidden factory that registers the ACP-supplied servers before bind. */
export function acpMcpExtension(servers: AcpMcpServer[], log: (line: string) => void = () => {}): InlineExtension {
  return {
    name: "zu-acp-mcp",
    hidden: true,
    factory: (pi) => {
      for (const server of servers) {
        if (typeof server?.name !== "string" || !server.name) {
          log("an ACP MCP server without a name was skipped");
          continue;
        }
        try {
          pi.registerMcpServer(server.name, toPiMcpConfig(server));
          if (server.type === "sse") log(`ACP MCP server ${server.name}: SSE is connected as HTTP`);
        } catch (failure) {
          log(`ACP MCP server ${server.name} was skipped: ${String(failure)}`);
        }
      }
    },
  };
}
