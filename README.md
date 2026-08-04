<p align="center">
  <a href="https://github.com/astrio-labs/forall">
    <picture>
      <source srcset="assets/forall_bg.png">
      <img src="assets/forall_bg.png" alt="Forall Background">
    </picture>
  </a>
</p>

<p align="center">
  <a href="./LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/License-Apache%202.0-blue.svg?style=flat-square" /></a>
  <a href="https://discord.com/invite/gESuZkdD5R"><img alt="Discord" src="https://img.shields.io/badge/Discord-community-5865F2?style=flat-square&logo=discord&logoColor=white" /></a>
  <a href="https://x.com/astriolabs"><img src="https://img.shields.io/badge/Follow%20on%20X-222021?logo=x&logoColor=white" alt="Follow on X"></a>
</p>

<div align="center">
  <img alt="Forall CLI" src="assets/forall-cli.png" width="800" />
</div>

## Why Forall exists

Review has always been the bottleneck in software. It is now the failure point,
because code is produced faster than anyone can check it. Tests sample a
handful of inputs. Types constrain shape. Neither tells you whether the code
does what it was supposed to do.

Formal verification answers that question, as far as your specification states
it, and it never failed on capability. Provers have been checking real programs
for years. What kept them out of ordinary engineering was adoption cost.
Writing the specifications, contracts and invariants a prover needs was more
tedious than writing the code itself. That is precisely the work language
models are now good at. The barrier was labour, and labour just got cheap.

Evidence can therefore become a build artifact rather than something
reconstructed by hand long after the code is done. Regulated work already
depends on it. Standards such as IEC 62304 and DO-178C require traceability
from each requirement to the verification that discharges it. Mission-critical
code should carry that evidence the way a paper carries its references,
attached to the specific claim it supports.

A green mark that lies is worse than no mark. A proof is only as strong as the
contract it discharges, and property testing only samples. Those differences
matter, so Forall reports four levels of evidence instead of a pass and a fail,
grading every requirement by the strongest evidence a machine actually
produced, never by what was claimed.

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

We are expanding to more languages, and to filling in more of this table for the languages already listed, based on demand.

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
