import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";

const DEFAULT_MCP_URL = "https://mcp.forall.astrio.app/mcp";

/**
 * Thin stdio → hosted Streamable HTTP bridge.
 * Requires FORALL_API_KEY from https://forall.astrio.app/dashboard
 */
export async function main() {
  const apiKey = process.env.FORALL_API_KEY?.trim();
  if (!apiKey) {
    throw new Error(
      "FORALL_API_KEY is required. Create one at https://forall.astrio.app/dashboard",
    );
  }

  const mcpUrl = new URL(process.env.FORALL_MCP_URL?.trim() || DEFAULT_MCP_URL);
  const loopback = ["localhost", "127.0.0.1", "[::1]", "::1"].includes(
    mcpUrl.hostname,
  );
  // The API key travels as a bearer header, so refuse any endpoint that would
  // put it on the wire in cleartext.
  const secure =
    mcpUrl.protocol === "https:" || (mcpUrl.protocol === "http:" && loopback);
  if (!secure) {
    throw new Error(
      `FORALL_MCP_URL uses ${mcpUrl.protocol}; https is required so the API key is not sent in cleartext`,
    );
  }

  const remote = new Client({ name: "forall-mcp-bridge", version: "0.1.0" });
  const transport = new StreamableHTTPClientTransport(mcpUrl, {
    requestInit: {
      headers: {
        Authorization: `Bearer ${apiKey}`,
      },
    },
  });
  await remote.connect(transport);

  const local = new Server(
    { name: "forall-mcp", version: "0.1.0" },
    { capabilities: { tools: {} } },
  );

  local.setRequestHandler(ListToolsRequestSchema, async () => {
    const listed = await remote.listTools();
    return { tools: listed.tools };
  });

  local.setRequestHandler(CallToolRequestSchema, async (request) => {
    return remote.callTool({
      name: request.params.name,
      arguments: request.params.arguments ?? {},
    });
  });

  const stdio = new StdioServerTransport();
  await local.connect(stdio);
}
