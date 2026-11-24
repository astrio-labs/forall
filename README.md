<p align="center">
<strong>Forall (∀)</strong> is Astrio’s coding agent for building provably correct software.
</p>

<div align="center">
  <img alt="Forall CLI" src="assets/forall-cli.png" width="800" />
</div>

<p align="center">
  <a href="./LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/License-Apache%202.0-blue.svg?style=flat-square" /></a>
  <a href="https://discord.com/invite/gESuZkdD5R"><img alt="Discord" src="https://img.shields.io/badge/Discord-community-5865F2?style=flat-square&logo=discord&logoColor=white" /></a>
  <a href="https://x.com/astriolabs"><img src="https://img.shields.io/badge/Follow%20on%20X-222021?logo=x&logoColor=white" alt="Follow on X"></a>
</p>

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

### 1. Build the Forall CLI

The terminal agent is derived from OpenAI Codex. It can edit your repository,
author specs and contracts, and run local verification.

On Linux, install the build dependencies and Bubblewrap for sandboxed commands
before building. See the [platform prerequisites](docs/getting-started.md).

```bash
git clone https://github.com/astrio-labs/forall.git
cd forall/agent
cargo build --locked -p forall-cli --bin forall
export PATH="$PWD/target/debug:$PATH"
```

In the Git project you want to work on, run `forall init` and then `forall`.
Choose **Bring your own API key** to use your model provider. Run `forall check`
to check the project locally. See the [build and proof-tool prerequisites](docs/getting-started.md).

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

## Connect

Join our [Discord](https://discord.com/invite/gESuZkdD5R) and [X](https://x.com/astriolabs) communities.

## License

This repository is licensed under the [Apache-2.0 License](LICENSE).
