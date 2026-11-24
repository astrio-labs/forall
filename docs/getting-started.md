# Getting started with Forall

Choose one path:

## 1. Forall CLI (full coding agent)

Install Rust through rustup, Git, and a native C/C++ toolchain. On macOS, use
the Xcode Command Line Tools. On Linux, the source build uses CMake, pkg-config
and OpenSSL development headers. Sandboxed commands also need a system
`bwrap` with `--perms` support and permission to create user namespaces. On
Ubuntu or Debian, install the prerequisites with the following command.

```bash
sudo apt-get install build-essential cmake pkg-config libssl-dev bubblewrap
```

The `agent/` directory pins the Rust toolchain used for the CLI.

```bash
git clone https://github.com/astrio-labs/forall.git
cd forall/agent
cargo build --locked -p forall-cli --bin forall
export PATH="$PWD/target/debug:$PATH"
forall --version
```

On Linux, run `forall sandbox -- /bin/echo sandbox-ok` to check that the
source-built CLI can launch a sandboxed command.

Run these commands in the Git repository you want Forall to edit.

```bash
forall init
forall
```

Choose **Bring your own API key** in the terminal. A local run can use your
provider credentials without an Astrio account. Use `forall exec` for a
non-interactive session.

### Local verification

Run `forall check` from the initialized project. For TypeScript contracts,
install Node.js and the workflow's JavaScript dependencies with `npm ci` in
`agent/workflow/` in the source checkout. Keep that checkout in place because the CLI also loads
its property-test runners from it. LemmaScript uses Dafny for proofs.
Other languages use their corresponding verification tools, including Verus
for Rust, OpenJML for Java, and Frama-C for C.

See [Project Layout](project-layout.md) and [Workflow](workflow.md).

### BYOK model providers

In the interactive CLI, run `/provider` and choose a provider. Paste the key into
the masked field and press Enter. If a key is already configured locally, press
Enter on an empty field to keep it. Azure also needs your resource endpoint.
Saving opens a new chat with the provider. To return to an earlier chat,
select its provider and run `/resume`.

Forall saves entered keys in its configured credential store. A key you enter
takes precedence over the same provider's shell or dotenv key. You can also
supply credentials through `~/.forall/.env` or your shell. Amazon Bedrock
uses AWS credentials configured separately.

| Provider | Configuration |
|----------|---------------|
| OpenAI | `OPENAI_API_KEY` |
| OpenRouter | `OPENROUTER_API_KEY` |
| Anthropic (Claude) | `ANTHROPIC_API_KEY` |
| Google Gemini | `GEMINI_API_KEY` |
| Azure OpenAI | `AZURE_OPENAI_API_KEY` + `AZURE_OPENAI_BASE_URL` |
| Amazon Bedrock (Claude) | Standard AWS credentials (profile / region); Bedrock API keys are not supported |

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
