<div align="left">

<h1>Forall (∀)</h1>

<p>Forall is a coding agent from Astrio that helps developers build correct software by generating spec-driven code alongside machine-checkable proofs.</p>

<p>
  <a href="./LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/License-Apache%202.0-blue.svg?style=flat-square" /></a>
  <a href="https://discord.com/invite/gESuZkdD5R"><img alt="Discord" src="https://img.shields.io/badge/Discord-community-5865F2?style=flat-square&logo=discord&logoColor=white" /></a>
</p>

<img alt="Forall CLI" src="assets/forall-cli.png" width="800" />

</div>

## Two ways to use Forall

### 1. Install Forall CLI

Full coding agent — specs, proofs, and workflow in your terminal.

```bash
curl -fsSL https://forall.astrio.app/install.sh | bash
forall
```

Add `~/.local/bin` to your `PATH` if needed, then run `forall --version`.

On first launch, sign in with your [Forall account](https://forall.astrio.app) — your browser opens, no API key to paste. Chat on your plan's hosted models, or bring your own model API key (OpenAI, OpenRouter, Anthropic (Claude), Google Gemini, Azure OpenAI, or Claude via Amazon Bedrock). Then `forall init` in a git repo and start working.

> **Note:** A binary release must exist on [GitHub Releases](https://github.com/astrio-labs/forall/releases) before install succeeds.

### 2. MCP verify-only

Stay on Cursor, Claude Code, or any MCP client — add hosted verification via MCP. **Do not** install the CLI.

1. Create an API key at [forall.astrio.app/dashboard](https://forall.astrio.app/dashboard)
2. Add to your MCP client:

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

Your coding agent edits the workspace from verify reports. See [docs/getting-started.md](docs/getting-started.md) and [docs/hosted-mcp.md](docs/hosted-mcp.md).

## Supported programming languages

Forall grades every requirement by the strongest evidence a machine actually
produced for it. Not every rung is reachable in every language, because the
rung depends on the tool behind it:

| | Spec tracked | Property tested | Contracted | Proved |
| --- | :---: | :---: | :---: | :---: |
| **TypeScript** | ✓ | ✓ | ✓ | ✓ |
| **Python** | ✓ | ✓ | ✓ | ✗ |
| **Rust** | ✓ | ✗ | ✓ | ✓ |
| **Java** | ✓ | ✗ | ✓ | ✓ |
| **C** | ✓ | ✗ | ✓ | ✓ |

- **Spec tracked** — a requirement cites this code.
- **Property tested** — a generator ran over many inputs and found no
  counterexample. Statistical evidence, honestly ranked below a proof.
- **Contracted** — a machine-checkable contract is written against the code,
  but nothing has confirmed it yet.
- **Proved** — a prover discharged that contract's obligations.

`Proved` comes from a prover, one per language: LemmaScript → Dafny for
TypeScript, Verus for Rust, OpenJML for Java, and Frama-C for C. Python has no
prover, so property tests are its strongest rung.

`Property tested` runs on the bundled Node and Python runners, which is why it
is available for TypeScript and Python only. The generator library itself —
fast-check or Hypothesis — is your project's own dependency.

We are expanding to more languages, and to more rungs within the languages
already listed, based on demand.

## Telemetry

First-party builds send product usage analytics — feature and reliability events, never your code or prompts. Opt out any time in `~/.forall/config.toml`:

```toml
[analytics]
enabled = false
```

## Connect

Join our [Discord](https://discord.com/invite/gESuZkdD5R) and [X](https://x.com/astriolabs) communities.

## License

This repository is licensed under the [Apache-2.0 License](LICENSE).
