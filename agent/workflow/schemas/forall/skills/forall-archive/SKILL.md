---
name: forall-archive
description: Archive a completed Forall change after successful native verification.
license: MIT
compatibility: Requires forall CLI.
metadata:
  author: forall
  version: "2.0"
---

Archive a completed change.

**Input**: Optionally a change name.

**Steps**

1. Call `author.status`, then `author.validate`. Stop if local validation fails.
2. Check the latest native verification with `verify.status`. If there is no
   successful terminal run for the current inputs, use `/forall:verify`.
3. Archive only after native verification succeeds.
4. **Archive**

   ```bash
   forall archive "<name>"
   ```

   This runs check, merges `mapping.delta.yaml` into `.forall/verified/mapping.yaml`, and moves the change to `.forall/changes/archive/YYYY-MM-DD-<name>/`.

5. **Summarize**

   ```
   ## Archive Complete

   **Change:** <name>
   **Archived to:** .forall/changes/archive/YYYY-MM-DD-<name>/
   **Mapping:** merged into .forall/verified/mapping.yaml
   **Verification:** Forall verified
   ```

**Guardrails**
- Do not archive without successful native verification unless the user
  explicitly overrides.
- Do not skip mapping merge — traceability lives in `mapping.yaml`
- Confirm when tasks or artifacts are incomplete.
- External MCP is optional and is not a prerequisite for standalone use.
