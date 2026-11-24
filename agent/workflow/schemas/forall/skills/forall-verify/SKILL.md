---
name: forall-verify
description: Validate a Forall change locally, submit native verification, and explain the result.
license: MIT
compatibility: Requires forall CLI.
metadata:
  author: forall
  version: "2.0"
---

Verify that a change is ready to archive.

**Steps**

1. Call `author.status` to confirm the intended project and authored scope.
2. Call `author.validate`. Do not submit while local validation has errors.
3. Call native `verify.submit` after validation succeeds.
4. Poll native `verify.status` until the run reaches a terminal state.
5. On failure, call `verify.explain`, fix the local inputs with `author.*`,
   validate again, and submit a new run.
6. Call `verify.cancel` only when a pending run is no longer wanted.
7. Report success as **Forall verified** or **machine-checked**.

An external MCP verification service is an optional transport for equivalent
operations; the standalone guidance must prefer native `verify.*`.

**Final rule**

- Local validation failure means **not ready to submit**.
- A failed or pending verification run means **not verified**.
- Only a successful terminal `verify.status` result is ready for archive.

**Guardrails**
- Never conflate `author.validate` with verification.
- Prefer actionable findings with requirement IDs and file paths.
- Do not expose implementation backend names in public reports.
- The check writes the verify ledger (`*-ledger.json`) — the per-requirement
  evidence record. Never edit it; it is sealed, and a hand-edited ledger
  reads as no evidence. Report proof status from the report and ledger, not
  from mapping claims.
