# Windows remote-environment test

This integration test exercises a Windows exec-server fixture under pinned Wine
and the normal model tool-call / remote-execution path.

It previously ran via Bazel targets in the legacy Codex monorepo. **This
repository no longer ships `BUILD.bazel` files** — CI and local development use
Cargo only. Re-enable this coverage when a Cargo-based Wine test harness exists.

## Current limitations

- ConPTY/TTY behavior is not yet covered.
- Wine loads shared objects and PE DLLs at runtime, so the host must still
  provide the declared compatible glibc version.
- The target is intentionally limited to x86-64 for simplicity.
