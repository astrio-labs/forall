# Forall workflow

Use the standalone Forall native tools for the complete workflow:

1. **Author locally** — call `author.status`, then `author.init` when needed. Use
   `author.discover`, `author.upsert_requirements`, `author.scaffold_contracts`,
   and `author.scaffold_property` to maintain requirements and evidence.
2. **Validate locally** — call `author.validate` after authoring and after code
   changes. Fix every validation error before requesting verification.
3. **Verify remotely** — call `verify.submit`, then poll `verify.status`. Use
   `verify.explain` for actionable failure details and `verify.cancel` only when
   the pending run is no longer wanted.

Native `author.*` and `verify.*` tools are preferred. An external MCP server may
expose equivalent tools, but it is optional and must not be required for the
standalone binary.

Keep implementation details private in normal user-facing output. Report
successful results as **Forall verified** or **machine-checked**. Do not claim
verification from local validation alone, and do not claim browser behavior
without browser evidence.

In the mapping, `verified: true` marks a requirement's SCOPE — precise logic
intended for formal proof during `forall check`. It is intent, not evidence.
Use `property_tested: true` only for explicitly requested or genuinely
property-based requirements. Keep presentation-only behavior spec-tracked.

Evidence is what `forall check` records in the verify ledger
(`*-ledger.json` beside the verify reports): per requirement, the level the
run actually earned, with the prover identity behind each verdict. Never
edit ledger files — they are written only by the check, carry an integrity
seal, and a hand-edited ledger reads as no evidence at all. When reporting
proof status, report what the ledger and verify report say, not what the
mapping claims.
