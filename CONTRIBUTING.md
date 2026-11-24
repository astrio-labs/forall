# Contributing

Thank you for your interest in contributing to Forall's public repository.

## What belongs here

The `agent/` Rust workspace builds the Forall CLI and terminal UI.
The root workspace maintains the libraries under `crates/`. Contributions
also cover documentation, skills, examples, and the MCP bridge.

To build the CLI, follow [Getting started](docs/getting-started.md). Run
`cargo test --workspace` at the repository root for the public libraries.
Run `cargo test --locked -p forall-workflow` from `agent/` for the workflow engine.

## How to contribute

1. Open an issue to discuss larger changes before opening a pull request.
2. Fork the repository and create a feature branch.
3. Keep changes focused and well-described.
4. Open a pull request with a clear summary of what changed and why.

## Pull request guidelines

- Follow existing file structure and naming conventions.
- Do not include secrets, API keys, or credentials.
- Do not add deploy workflows that require private infrastructure secrets.

## License

By contributing, you agree that your contributions are licensed under the Apache-2.0 license in [LICENSE](LICENSE).
