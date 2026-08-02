# Changelog

All notable changes to this public repository are documented here.

## Unreleased

- Security: verification snapshots no longer follow symlinked root manifests or a
  symlinked `.forall` directory, either of which could send files from outside
  the workspace to hosted verification.
- Security: the hosted client and the `@astrio/forall-mcp` bridge refuse a
  non-loopback `http://` endpoint so the bearer token is never sent in cleartext.
- Security: `install.sh` verifies a published `<asset>.sha256` when one exists,
  rejects release archives containing absolute or `..` member paths, and never
  chmods a symlink out of the unpack directory. Set `FORALL_REQUIRE_CHECKSUM=1`
  to refuse any download that cannot be verified.
- Security: workflow actions are pinned to commit SHAs, workflows run with
  `contents: read`, and Dependabot keeps actions, crates, and npm current.
- Contract scaffolding no longer corrupts signatures that contain a brace before
  the body, such as an object parameter, an object return type, or a generic bound.
- Symbol discovery keeps nested parentheses in signatures and finds generic
  functions in TypeScript and Rust.
- `install.sh` now exits when it cannot support the detected OS or architecture
  instead of continuing with an empty target.
- The security policy and issue template point at `astrio-labs`, not the retired
  `astrio-ai` namespace.
- `@astrio/forall-mcp` requires Node 20 or newer. Clearing two advisories in the
  MCP SDK's transitive dependencies pulled in a package that needs Node 20, and
  Node 18 has been end-of-life since April 2025.

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
