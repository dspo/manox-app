/**
 * A minimal MCP stdio server for live acceptance: one `echo` tool, newline
 * framed JSON-RPC. It exists to prove that ACP-supplied `mcpServers` reach
 * pi's MCP extension end to end; it is not part of the distro.
 */

type Message = { jsonrpc?: string; id?: number | string; method?: string; params?: any };

function reply(message: Message): void {
  process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", id: message.id, result: resultFor(message) })}\n`);
}

function resultFor(message: Message): unknown {
  switch (message.method) {
    case "initialize":
      return {
        protocolVersion: message.params?.protocolVersion ?? "2024-11-05",
        capabilities: { tools: {} },
        serverInfo: { name: "zu-fixture-mcp", version: "0.0.0" },
      };
    case "ping":
      return {};
    case "tools/list":
      return {
        tools: [
          {
            name: "echo",
            description: "Echo the given text.",
            inputSchema: {
              type: "object",
              properties: { text: { type: "string" } },
              required: ["text"],
            },
          },
        ],
      };
    case "tools/call": {
      const text = String(message.params?.arguments?.text ?? "");
      return { content: [{ type: "text", text }], isError: false };
    }
    case "resources/list":
      return { resources: [] };
    case "prompts/list":
      return { prompts: [] };
    default:
      return {};
  }
}

let buffer = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk: string) => {
  buffer += chunk;
  let newline = buffer.indexOf("\n");
  while (newline >= 0) {
    const line = buffer.slice(0, newline).trim();
    buffer = buffer.slice(newline + 1);
    if (line) {
      let message: Message;
      try {
        message = JSON.parse(line) as Message;
      } catch {
        newline = buffer.indexOf("\n");
        continue;
      }
      // A notification has no id and expects no reply.
      if (message.id !== undefined && message.id !== null) reply(message);
    }
    newline = buffer.indexOf("\n");
  }
});
