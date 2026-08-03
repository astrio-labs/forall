# Java contracts

Forall contracts are `//@` line comments **above** methods.

## Annotation style

```java
public class Clamp {
    //@ requires lo <= hi;
    //@ ensures lo <= \result && \result <= hi;
    public static int clamp(int x, int lo, int hi) {
        if (x < lo) return lo;
        if (x > hi) return hi;
        return x;
    }
}
```

Notes:

- Use `\result` for the return value
- Terminate contract clauses with `;`
- Prefer `public static` pure helpers for the first proved slice

## Mapping

```yaml
verified: true
code:
  file: src/Clamp.java
  symbols: [clamp]
```

## Proof scope (important)

Start with **scalar** methods:

- Prove one method at a time (`clamp`, `shareFor`, …)
- Keep array assembly, HTTP handlers, and multi-method orchestration
  **spec-tracked** (`verified: false`)

Avoid until core lemmas pass:

- Aggregate expressions over arrays
- Recursive prefix-sum style postconditions
- Heavy loop invariants tying multiple arrays together

## Good first targets

- Bounds / saturation
- Fee or share calculations on scalars
- Predicate helpers with explicit requires

## Failure loop

1. Read hosted `proofs` issues / `proof_detail`
2. Simplify ensures; split methods if needed
3. Keep hard array logic spec-tracked temporarily
4. Never flip `verified: true` → false to silence failures — the flag is proof scope; the ledger records what was actually earned either way
