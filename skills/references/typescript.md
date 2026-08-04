# TypeScript contracts

Forall contracts live in `.ts` and `.tsx` files as `//@` annotations.

## Annotation style

Place annotations **inside** the function:

```ts
export function clamp(x: number, lo: number, hi: number): number {
  //@ requires lo <= hi
  //@ ensures lo <= \result && \result <= hi
  //@ contract Bounds clamp
  if (x < lo) return lo;
  if (x > hi) return hi;
  return x;
}
```

Notes:

- Use `\result` for the return value in ensures. The leading backslash is
  required, and a bare `result` is parsed as an ordinary identifier, so the
  proof fails with `unresolved identifier: result` rather than a syntax error
- Keep predicates decidable and local to the function
- Avoid I/O, DOM, and network inside verified functions

## Mapping

```yaml
verified: true
code:
  file: src/clamp.ts
  symbols: [clamp]
```

## Good first targets

- Bounds clamping, saturation arithmetic
- Parsers with finite grammars
- Validators / policy checks with clear preconditions
- Pure scoring / ranking helpers

## Avoid on first pass

- React components and hooks
- Fetch / filesystem wrappers
- Functions whose specs need unbounded quantification (use PBT or split)

## Failure loop

1. Read `proofs` phase issues from hosted status
2. Tighten `//@ requires` / `//@ ensures` or fix the implementation
3. Re-verify — `verified` marks proof scope; evidence lives in the sealed verify ledger, so flipping the flag changes intent, never outcomes
