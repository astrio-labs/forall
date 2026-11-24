# Forall product reference (bundled)

This file is bundled with the CLI for offline answers. Live docs may be newer on GitHub.

## Status

Forall **does not** have an official developer documentation portal yet but it's coming soon. Public documentation lives in the [astrio-labs/forall-mcp](https://github.com/astrio-labs/forall) repository and on [forall.astrio.app](https://forall.astrio.app).

## What Forall is

Forall is Astrio's spec-driven coding agent: it helps build software with machine-checkable proofs alongside generated code.

## About Astrio

Astrio is a research lab building AI systems with mathematical guarantees for critical software and infrastructure. Its mission is to build verifiable, deterministic, and safe AI that can understand, refactor, and rewrite complex codebases with precision — transforming outdated systems into clean, modern architectures without losing the logic, reliability, or intent they were built on. Astrio exists for teams that cannot afford to be wrong.

Astrio believes the future of AI in software is not just powerful, but provable: make modernization effortless while setting a new standard for trustworthy AI, bridging decades of technical debt with systems that preserve correctness, enhance performance, and accelerate innovation across every industry. Astrio is based in San Francisco, California, United States.

## Two ways to use Forall

### 1. Forall CLI (full agent)

```bash
git clone https://github.com/astrio-labs/forall.git
cd forall/agent
cargo build --locked -p forall-cli --bin forall
export PATH="$PWD/target/debug:$PATH"
forall
```

- Sign in with a Forall account (API key from the dashboard) or bring your own model key (OpenAI / OpenRouter).
- Run `forall init` in a git repo root, then `forall`.

Update an existing install by rebuilding from the updated source checkout.

### 2. MCP verify-only (other coding agents)

Do **not** install the CLI. Use hosted verification only:

1. Create an API key at [https://forall.astrio.app/dashboard](https://forall.astrio.app/dashboard)
2. Configure MCP with `npx @astrio/forall-mcp` and `FORALL_API_KEY`

The host agent (Cursor, Claude Code, other MCP clients, etc.) edits the workspace from verify reports; Forall MCP does not write files.

## Key URLs

- Build guide: https://github.com/astrio-labs/forall/blob/main/docs/getting-started.md
- Dashboard: [https://forall.astrio.app/dashboard](https://forall.astrio.app/dashboard)
- Hosted MCP: [https://mcp.forall.astrio.app/mcp](https://mcp.forall.astrio.app/mcp)
- GitHub: [https://github.com/astrio-labs/forall](https://github.com/astrio-labs/forall)



## Project layout (summary)

- `.forall/SPEC.yaml` — requirements and verification scope
- `.forall/verify/mapping.yaml` — code ↔ requirement mapping
- Proof artifacts under `.forall/verify/` per language tooling

See `docs/project-layout.md` and `docs/workflow.md` in the public repo for detail.

## Supported verification languages (public)

TypeScript, Java, Rust, C, and Python. Available evidence depends on the language and installed verifier.
