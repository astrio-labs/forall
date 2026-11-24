---
name: forall-apply
description: Implement a locally authored Forall change, keep contracts synchronized, and validate before verification.
license: MIT
compatibility: Requires forall CLI.
metadata:
  author: forall
  version: "2.0"
---

Implement tasks from a Forall change.

**Steps**

1. Call `author.status` and read the authored requirements and mapped symbols.
2. If mappings are incomplete, use `author.discover`,
   `author.upsert_requirements`, and `author.scaffold_contracts` before editing.
3. Implement focused code changes. Keep mapped contracts and property evidence
   synchronized through native `author.*` operations.
4. Call `author.validate` after edits and resolve every local validation error.
5. When implementation is complete and local validation succeeds, run
   `/forall:verify`.

**Guardrails**
- Read requirements and mappings before editing code.
- Keep changes minimal and preserve user-authored content.
- Do not weaken requirements or add assumptions to bypass verification.
- Local `author.validate` success is required before native `verify.submit`.
- External MCP authoring is optional; standalone native tools are sufficient.
