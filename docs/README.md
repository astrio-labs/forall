# Forall Documentation

| Guide | What it covers |
|-------|----------------|
| [Getting Started](getting-started.md) | Two paths: Forall CLI vs MCP verify-only |
| [Project Layout](project-layout.md) | The `.forall/` directory and what each file does |
| [Workflow](workflow.md) | The propose → verify → archive change workflow |
| [Architecture](architecture.md) | Two-path product model and open-source components |
| [Hosted Forall MCP](hosted-mcp.md) | Remote verification, dashboard keys, npm bridge |

## Packages

- [`packages/forall-mcp`](../packages/forall-mcp/README.md) — `@astrio/forall-mcp` npm bridge

## Open-source crates

The [agent workspace](../agent/) builds the CLI and terminal UI.
The public authoring, verification client, and protocol libraries are listed in
[crates/README.md](../crates/README.md).
