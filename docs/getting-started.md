# Getting started with Forall

Choose one path:

## 1. Forall CLI (full coding agent)

Install and run Forall as your agent:

```bash
curl -fsSL https://forall.astrio.app/install.sh | bash
forall
```

On first launch, pick:

| Option | What happens |
|--------|-------------|
| **Forall account** | Your browser opens to sign in — then chat on your plan's hosted models, or add BYOK keys any time |
| **Bring your own API key** | Pick a provider and use keys from `~/.forall/.env` (matrix below) |

CLI equivalents:

```bash
forall login                                          # browser sign-in
printenv FORALL_API_KEY | forall verification login   # headless, API key
forall verification status
```

Then initialize a project from a git repo root:

```bash
forall init
forall
```

See [Project Layout](project-layout.md) and [Workflow](workflow.md).

### BYOK model providers

Set keys in `~/.forall/.env` (or export them in your shell), then pick the provider during onboarding. Amazon Bedrock is configured through `config.toml` rather than onboarding:

| Provider | Configuration |
|----------|---------------|
| OpenAI | `OPENAI_API_KEY` |
| OpenRouter | `OPENROUTER_API_KEY` |
| Anthropic (Claude) | `ANTHROPIC_API_KEY` |
| Google Gemini | `GEMINI_API_KEY` |
| Azure OpenAI | `AZURE_OPENAI_API_KEY` + `AZURE_OPENAI_BASE_URL` |
| Amazon Bedrock (Claude) | Standard AWS credentials (profile / region); Bedrock API keys are not supported |

### Telemetry

First-party builds send product usage analytics — feature and reliability events, never your code or prompts. Opt out in `~/.forall/config.toml`:

```toml
[analytics]
enabled = false
```

### Supported platforms

| OS | Architectures |
|----|---------------|
| macOS | Apple Silicon (`aarch64`), Intel (`x86_64`) |
| Linux | `x86_64`, `aarch64` |
| Windows | `x86_64` |

## 2. MCP verify-only (Cursor / Claude Code / Codex)

Do **not** install the Forall CLI. Use the npm bridge with your existing agent:

1. Create a key at [forall.astrio.app/dashboard](https://forall.astrio.app/dashboard)
2. Configure MCP:

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

Hosted MCP only verifies. Your coding agent edits the workspace from the report.

See [Hosted Forall MCP](hosted-mcp.md), [packages/forall-mcp](../packages/forall-mcp/README.md), and
[Architecture](architecture.md).
