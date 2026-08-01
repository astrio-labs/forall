# Architecture

Forall has two supported product paths. There is no Hybrid / local-authoring MCP
path for external agents.

```text
Path A — Forall CLI                         Path B — MCP verify-only
───────────────────                         ────────────────────────
install.sh → forall binary                  npm: @astrio/forall-mcp
TUI: Forall account OR BYOK                 dashboard API key in MCP config
native authoring + workflow in-process      stdio → hosted HTTP MCP
optional hosted verify via Forall API key   verify tools only (no workspace writes)
```

See [getting-started.md](getting-started.md).

```text
┌────────────────────────────────────────────────────────────┐
│  Path A: `forall` binary (closed-source distribution)    │
│  agent TUI · native tools · sandbox · workflow commands    │
└────────────────────────────┬───────────────────────────────┘
                             │
                             ▼
┌────────────────────────────────────────────────────────────┐
│  native author.*   deterministic workspace operations      │
│  native verify.*   hosted submit / status / explain        │
│  (embed open-source crates in-process)                     │
└───────────────┬───────────────────────────┬────────────────┘
                │                           │
                ▼                           ▼
┌──────────────────────────────┐  ┌───────────────────────────┐
│ Project workspace            │  │ Hosted Forall MCP         │
│ .forall/ + application code  │  │ isolated verification     │
└──────────────────────────────┘  └───────────────────────────┘

Path B: Cursor / Claude Code / Codex
    npx @astrio/forall-mcp + FORALL_API_KEY
         │
         ▼
    https://mcp.forall.astrio.app/mcp  →  same hosted workers
    (host agent edits the repo from verify reports)
```

## Auth

| Audience | Credential | Where it is created / stored |
|----------|------------|------------------------------|
| CLI — Forall account | Browser sign-in via `forall login` (OAuth) | Hosted models on your plan; a `forall_…` API key still works headless via `forall verification login` |
| CLI — BYOK | `OPENAI_API_KEY` / `OPENROUTER_API_KEY` / `ANTHROPIC_API_KEY` / `GEMINI_API_KEY` / `AZURE_OPENAI_API_KEY` / AWS credentials (Bedrock) | `~/.forall/.env` or `forall login --with-api-key` |
| MCP verify-only | `FORALL_API_KEY` | Same dashboard → MCP client `env` |

Model chat runs on your Forall plan's hosted models or on BYOK provider keys. A
Forall account also unlocks hosted verification and account-tied quotas.

## Open-source components

| Component | Location | Role |
| --- | --- | --- |
| Authoring library | `crates/forall-authoring` | Safe status, init, discovery, mapping, contracts, validation |
| Hosted client | `crates/forall-hosted-verify` | Snapshot packaging and authenticated submit/status/cancel/explain |
| npm MCP bridge | `packages/forall-mcp` | Stdio → hosted MCP for external agents (`@astrio/forall-mcp`) |

The full agent runtime (TUI, turn loop, sandbox, native tool registry) is
distributed only as the prebuilt `forall` binary.

## Hosted verification

- **CLI users** call native `verify.*` tools (or `forall verification login`).
  They should not manually add the hosted MCP URL to Forall.
- **External agents** use `@astrio/forall-mcp` (or HTTP MCP with Bearer) only.

See [hosted-mcp.md](hosted-mcp.md).

## Project layout

`forall init` writes shareable files into the repo:

```text
.forall/
├── markers.toml
├── workflow/
│   ├── config.yaml
│   ├── active
│   ├── changes/<name>/
│   └── archive/
├── verify/
│   ├── mapping.yaml
│   └── cache/
├── specs/
└── scenarios/
```

Machine-local state lives in `~/.forall/`.

## Spec workflow

```text
proposal → specs → design → verification → tasks → apply → archive
```

See [workflow.md](workflow.md).

## Out of scope

- Local authoring MCP (`forall mcp-author`) for external agents — not a product
  path. Authoring for other agents stays with those agents; Forall MCP is
  verify-only.
