# Verification plan: {{CHANGE_NAME}}

## Machine-checked requirements

For each requirement with precise rules, mark `verified: true` in `mapping.delta.yaml` and plan:

| Requirement id | Capability | File | Symbols | Contract sketch |
|----------------|------------|------|---------|-----------------|
| | | | | |

Use native `author.discover`, `author.upsert_requirements`, and
`author.scaffold_contracts` during implementation. Run `author.validate`
locally before native `verify.submit`; poll `verify.status` for the
machine-checked result. Equivalent external MCP tools are optional.

## Property-based tests (optional — not the default)

Use `property_tested: true` **only** when the user asked for property testing,
or quantified infinite-domain properties are a better fit than machine
checking:

| Requirement id | Property file | Symbol | Why PBT instead of proof |
|----------------|---------------|--------|--------------------------|
| | `.forall/scenarios/<id>.property.ts` | | |

Create property evidence with `author.scaffold_property`, then rerun
`author.validate`.

`verified` and `property_tested` are **mutually exclusive** per requirement.

## Mapping delta

See `mapping.delta.yaml` in this change directory.
