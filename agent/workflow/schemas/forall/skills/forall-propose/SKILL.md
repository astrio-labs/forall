---
name: forall-propose
description: Author requirements and contracts for a Forall change, validate them locally, and prepare the change for native verification.
license: MIT
compatibility: Requires forall CLI.
metadata:
  author: forall
  version: "2.0"
---

Author a change through native tools.

1. Call `author.status`; call `author.init` if the project is not initialized.
2. Call `author.discover` for relevant source files or languages.
3. Translate the request into stable requirement IDs and call
   `author.upsert_requirements` in preview mode, then apply the reviewed result.
4. Call `author.scaffold_contracts` for mapped symbols. Use
   `author.scaffold_property` only where property evidence is appropriate.
5. Call `author.validate` and fix all errors locally.
6. Once local validation succeeds, hand off to `/forall:verify`, which uses
   native `verify.*` tools.

An external MCP server exposing equivalent authoring operations is optional;
the standalone native tools are the default.

**Output**

Summarize:
- Requirements and source symbols authored
- Files created or updated
- Local validation result
- Whether the change is ready for native verification

**Guardrails**
- Preview mutations before applying them.
- Preserve caller-owned content and honor stale-content errors.
- Never submit verification before `author.validate` succeeds.
- Do not expose implementation backend names in normal user-facing output.
