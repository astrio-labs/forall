---
name: forall-continue
description: Continue local Forall authoring from project status through validation.
license: MIT
compatibility: Requires forall CLI.
metadata:
  author: forall
  version: "2.0"
---

Continue authoring through native tools.

1. Call `author.status` and inspect current requirements, languages, mappings,
   and warnings.
2. Use `author.discover` when source symbols need refreshing.
3. Complete the next missing authoring operation with
   `author.upsert_requirements`, `author.scaffold_contracts`, or
   `author.scaffold_property`. Preview before apply.
4. Call `author.validate`. Fix local errors before suggesting implementation or
   native verification.
5. If validation succeeds and implementation is complete, suggest
   `/forall:verify`.

Equivalent external MCP tools may be used when already configured, but they are
optional.

**Guardrails**
- Preserve existing project content.
- Ask before guessing ambiguous requirements.
- Do not treat local validation as completed verification.
- Keep implementation backend names out of public summaries.
