# Open-source crates

Rust libraries embedded by the `forall` binary and published in this repository.

The [agent workspace](../agent/) contains the CLI and terminal UI.

## Crates

| Crate | Purpose |
| --- | --- |
| [`forall-authoring`](forall-authoring/) | Deterministic workspace authoring: project status, initialization, symbol discovery, requirement mapping, contract scaffolding, property scaffolding, and validation |
| [`forall-hosted-verify`](forall-hosted-verify/) | Hosted verification MCP client, snapshot packer, and wire types |
| [`forall-wire`](forall-wire/) | Shared verification protocol types |

## Building and testing

```bash
cargo test --workspace
```

## Documentation

- [Architecture](../docs/architecture.md)
- [Hosted Forall MCP](../docs/hosted-mcp.md)
