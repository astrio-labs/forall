# `@astrio/forall-mcp`

Stdio MCP bridge to Forall's **hosted verification** service. For Cursor, Claude Code, and other MCP clients that speak stdio — not the Forall CLI.

This package does **not** edit your workspace. It only proxies verify tools to `https://mcp.forall.astrio.app/mcp`. Your coding agent applies fixes from the reports.

## Setup

1. Create an API key at [forall.astrio.app/dashboard](https://forall.astrio.app/dashboard)
2. Add the MCP server to your client:

```json
{
  "mcpServers": {
    "forall": {
      "command": "npx",
      "args": ["-y", "@astrio/forall-mcp"],
      "env": {
        "FORALL_API_KEY": "forall_..."
      }
    }
  }
}
```

## Environment

| Variable | Required | Default |
|----------|----------|---------|
| `FORALL_API_KEY` | yes | — |
| `FORALL_MCP_URL` | no | `https://mcp.forall.astrio.app/mcp` |

## vs Forall CLI

| | CLI | This package |
|--|-----|--------------|
| Install | `curl …/install.sh \| bash` | `npx @astrio/forall-mcp` |
| Audience | Users adopting Forall as their agent | Users staying on Cursor / Claude / other hosts |
| Auth | BYOK and/or Forall OAuth + dashboard key in TUI | Dashboard API key only |
| Workspace writes | Yes (Forall agent) | No |

## Tools

Proxied from the hosted service (names may evolve):

- `forall_verify`
- `forall_verification_status`
- `forall_cancel_verification`
- `forall_explain_verification`
