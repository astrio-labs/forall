# Changelog

All notable changes to this public repository are documented here.

## Unreleased

## v0.5.0

- Sign in with GitHub: `forall login` gains GitHub alongside the browser flow — no API-key paste required.
- Connect a repository once and Forall mints repo-scoped git credentials; agent commits are attributed to your GitHub identity.
- Forall-branded sign-in success and error pages across the OAuth flows.
- Verification: proof failures are attributed to the failing file for clearer diagnostics.
- Spec and proof tool calls are recorded in the thread and replay on `thread/resume`.
- Model catalog: add the GPT-5.6 family (Sol, Terra, Luna).
- Privacy: Forall account tokens are sent only to Forall-hosted models, never to bring-your-own-key providers.
- Show the account's display name across the product.
- Document C as a supported language (Frama-C/WP proof backend, shipped in v0.4.0).
- Open-source crates verified in sync with `astrio-labs/forall-core` (`agent/forall-hosted-verify`, `agent/workflow/src/authoring`) as of 2026-08-02.

## v0.4.0

- Sign in with your Forall account: `forall login` opens the browser — no API-key paste required.
- Forall plans include hosted models; bring-your-own-key stays fully supported.
- Native Anthropic support: Claude models via `ANTHROPIC_API_KEY` (Messages API adapter).
- Native Google Gemini support via `GEMINI_API_KEY` (generateContent adapter).
- Azure OpenAI support via `AZURE_OPENAI_API_KEY` + `AZURE_OPENAI_BASE_URL`.
- Claude on Amazon Bedrock using standard AWS credentials (Bedrock API keys are not supported).
- Privacy: streaming traces record event structure only — chunk bodies are never logged, and wire-supplied labels are bounded.
- Document product telemetry and the `[analytics] enabled = false` opt-out.
- Getting-started gains a BYOK provider matrix; auth tables refreshed.
- `@astrio/forall-mcp` source mirror synced (description refresh).
- Open-source crates verified in sync with `astrio-labs/forall-core` (`agent/forall-hosted-verify`, `agent/workflow/src/authoring`) as of 2026-07-31.

## v0.3.0

- Remove apps, connectors, and remote control from the CLI.
- Auth is `FORALL_API_KEY` / BYOK API keys only.
- Refresh stale update-version cache when the running CLI is ahead of cached latest.
- Ship prebuilt macOS, Linux, and Windows release archives.

## v0.2.1

- Add a Feature request GitHub issue template.
- Hide verification success cards on chat-only turns.
- Clarify Specs tracked copy vs formal Verification passed in the TUI.

## v0.2.0

- Prebuilt CLI binaries published as gzip-compressed GitHub Release archives.
- Add open-source adapter crates: `forall-authoring` and `forall-hosted-verify`.
- Add hosted MCP authoring skills and language contract references.
- Hide verification implementation details from public skills.
- Rewrite docs and README for the two-path model: Forall CLI vs `@astrio/forall-mcp` verify-only.
- Remove deprecated Hybrid local-authoring MCP (`forall-mcp-author` crate and skill).
- Install from gzip-compressed release archives, with fallback to raw binaries for older tags.
- Point `install.sh` at `astrio-labs/forall` and hint at the MCP verify-only path.
- Add `packages/forall-mcp` (`@astrio/forall-mcp` npm bridge source mirror).
- Open-source crates verified in sync with `astrio-labs/forall-core` (`agent/forall-hosted-verify`, `agent/workflow/src/authoring`) as of 2026-07-16.
- Expand CI with packages checks, docs link verification, Rust fmt/clippy gates, and stronger release smoke tests.

## v0.1.0

- Initial public tree: installer, documentation, and community assets.
- Prebuilt CLI binaries published via GitHub Releases (not built from this repo).
- Add user-facing docs: getting started, project layout, and workflow.
- Add brand/CLI screenshot assets and a centered README hero.
- Point the install command at `https://forall.astrio.app/install.sh`.
- Add a `.forall/` project skeleton at the repo root.
