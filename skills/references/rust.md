# Rust contracts

Forall contracts live inline in `.rs` files.

## Annotation style

Prefer named results so ensures clauses can mention `result`:

```rust
pub fn clamp(x: u64, lo: u64, hi: u64) -> (result: u64)
    requires
        lo <= hi,
    ensures
        lo <= result && result <= hi,
{
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}
```

Shorter form also works for simple cases:

```rust
pub fn clamp(x: u64, lo: u64, hi: u64) -> u64
    requires lo <= hi,
    ensures lo <= result && result <= hi,
{
    if x < lo { lo } else if x > hi { hi } else { x }
}
```

Add `proof { ... }` blocks only when needed for non-trivial reasoning.

When submitting inline files to hosted MCP, include `Cargo.toml` and the mapped
crate sources.

## Mapping

```yaml
verified: true
code:
  file: src/clamp.rs
  symbols: [clamp]
```

## Good first targets

- Pure numeric helpers
- Invariants on IDs, ranges, and enums
- Small state-transition functions with explicit preconditions

## Avoid on first pass

- `async` / Tokio glue
- FFI and `unsafe` without a narrow verified core
- Large modules — extract a pure helper and map that instead

## Failure loop

1. Fix `requires` / `ensures` or the body
2. Keep proofs honest — no unproven `assume`
3. Re-run hosted `forall_verify`
4. Do not downgrade `verified: true` because proofs are hard (the ledger records earned outcomes regardless); simplify the
   contract or shrink the verified surface
