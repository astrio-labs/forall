---
name: "forall-docs"
description: "Use when the user asks about Forall itself — install, CLI vs MCP verify-only, dashboard/API keys, project layout, workflow, hosted verification, architecture, or troubleshooting. Forall does not have an OpenAI-style developer portal yet; use the bundled helper and public GitHub docs first."
---

# Forall docs

Answer questions about **Forall the product**. Unlike OpenAI or legacy Codex, Forall **does not** publish an official developer docs site yet (no `developers.openai.com`-equivalent). Do not invent one or fetch Codex/OpenAI Forall-manual URLs.

## Source priority

1. **Bundled reference** — `references/product.md` in this skill (stable product facts).
2. **Public GitHub docs** — run the helper to cache current markdown from [astrio-labs/forall-mcp](https://github.com/astrio-labs/forall).
3. **Session facts** — `forall --version`, `forall doctor`, config under `~/.forall/`, project `.forall/`, and tools available in the current session.
4. **Bounded uncertainty** — say what is not documented publicly yet; do not guess.

For **OpenAI API / model provider** questions (BYOK only), point to [platform.openai.com](https://platform.openai.com/docs) or the user's configured provider docs. That is separate from Forall product docs.

## When to use this skill

- Install, update, auth, dashboard keys, CLI vs MCP paths
- `.forall/` project layout, workflow, verification, hosted MCP
- Skills, plugins, hooks, MCP config as documented in the public repo
- “What is Forall?” / “How do I use Forall with Cursor?”

Do **not** use this skill for generic coding tasks unrelated to Forall unless the user asks how Forall fits in.

## Fetch public docs (writable sessions)

When temp cache and shell are allowed:

```bash
node <skill-dir>/scripts/fetch-forall-docs.mjs
```

- `<skill-dir>` is this skill's directory (installed copy: `$FORALL_HOME/skills/.system/forall-docs`).
- Default cache: `$TMPDIR/forall-docs-cache` (or `%TEMP%\\forall-docs-cache` on Windows).
- Output: `forall-docs.md` plus `forall-docs.outline.md` with heading → line ranges.
- Refresh if older than ~1 day or docs look stale.

If the helper fails (read-only, no network, no shell), read `references/product.md` and state that live GitHub docs were unavailable.

## Canonical URLs (cite these)

| Topic | URL |
| --- | --- |
| Public repo + docs | https://github.com/astrio-labs/forall |
| Install CLI | https://github.com/astrio-labs/forall/blob/main/docs/getting-started.md |
| Dashboard / API keys | https://forall.astrio.app/dashboard |
| Hosted MCP endpoint | https://mcp.forall.astrio.app/mcp |
| npm MCP bridge | `@astrio/forall-mcp` |

## Two product paths (do not conflate)

| Path | Who | Install |
| --- | --- | --- |
| **CLI** | Forall as the coding agent | Build `forall-cli` with Cargo from `agent/` |
| **MCP verify-only** | Stay on Cursor / Claude / other hosts | `npx @astrio/forall-mcp` + `FORALL_API_KEY` from dashboard — **no CLI** |

Hybrid / local-authoring MCP is **not** a supported product path.

## Local configuration surfaces

| Surface | Scope |
| --- | --- |
| `~/.forall/config.toml` | Global Forall settings |
| `~/.forall/.env` | Provider API keys (BYOK) |
| Project `.forall/` | Spec, mapping, verify artifacts |
| `AGENTS.md` | Durable repo guidance for any agent |
| Skills / plugins | Reusable workflows and bundles |

## Output rules

- Prefer facts from fetched docs or `references/product.md`.
- Cite GitHub paths or URLs when answering product questions.
- If something is internal, unreleased, or undocumented, say so explicitly.
- Do not reference `openai-docs`, Codex manual URLs, or `developers.openai.com/codex/*` for Forall product answers.
