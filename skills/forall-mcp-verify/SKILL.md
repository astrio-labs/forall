---
name: forall-mcp-verify
description: >-
  Run hosted Forall verification via MCP (forall_verify, status, cancel,
  explain) without the Forall CLI. Use after authoring .forall mapping and
  proof contracts, or whenever the user asks to check / re-verify a project
  with the hosted Forall MCP server.
license: MIT
compatibility: >-
  Requires the Forall hosted MCP server configured with FORALL_API_KEY.
  Forall CLI is not required.
metadata:
  author: forall
  version: "1.0"
---

# Forall MCP verify (hosted check)

Submit verification to the hosted Forall MCP service and iterate on the report.

Endpoint: `https://mcp.forall.astrio.app/mcp`
Auth: `Authorization: Bearer <FORALL_API_KEY>`

## Tools

| Tool | Purpose |
|------|---------|
| `forall_verify` | Submit async job (inline files or public GitHub) |
| `forall_verification_status` | Poll progress + sanitized report |
| `forall_cancel_verification` | Cancel queued/running job |
| `forall_explain_verification` | Explain selected findings |

## Preconditions

Before verify:

1. `.forall/verify/mapping.yaml` exists (`version: 1`)
2. At least one requirement is mapped, or you accept a structure-only pass
3. `.forall/verify/mapping.yaml` exists and requirements have contracts (see
   `skills/references/`). Your host agent authors these — Forall MCP is verify-only.

If mapping is empty, hosted check may succeed with structure warnings only —
that is **not** useful verification. Author requirements first.

## Steps

### 1. Choose source

**GitHub** (preferred when the commit is public and pushed):

```json
{
  "source": {
    "type": "github",
    "repository": "owner/repo",
    "ref": "main"
  },
  "scope": { "type": "project" },
  "strict": false
}
```

Optional: `subdirectory` for monorepos.

**Inline** (local / private / unpushed work):

Include every path the check needs:

- `.forall/verify/mapping.yaml`
- mapped source files (`.ts` / `.tsx` / `.rs` / `.java`)
- `Cargo.toml` (+ crate sources) for Rust
- property-test files under `.forall/scenarios/` when `property_tested: true`

```json
{
  "source": {
    "type": "inline",
    "files": [
      { "path": ".forall/verify/mapping.yaml", "content": "..." },
      { "path": "src/clamp.ts", "content": "..." }
    ]
  },
  "scope": { "type": "project" },
  "strict": false
}
```

Change-scoped check:

```json
{
  "scope": { "type": "change", "name": "add-clamp-bounds" }
}
```

### 2. Submit

Call `forall_verify`. Save `job_id` and honor `poll_after_ms`.

### 3. Poll until terminal

Call `forall_verification_status` with `{ "job_id": "vrf_..." }`.

Terminal states: `succeeded`, `failed`, `cancelled`, `expired`.

Non-terminal: `queued`, `preparing`, `running` — wait and poll again.

### 4. Read the report

Focus on:

- `result.ok`
- `result.phases` (`structure`, `mapping`, `proofs`, …)
- `result.issues[]` with `severity`, `phase`, `file`, `requirement_id`,
  `message`, `proof_detail`, `counterexample`
- `result.verification_summary` (proved / property-tested / spec-tracked counts)

CRITICAL issues block a real verify claim. Fix them before telling the user
the project is machine-checked.

### 5. Explain opaque failures

```json
{
  "job_id": "vrf_...",
  "issue_indexes": [0, 1],
  "audience": "developer"
}
```

Use explanations to drive local edits, then re-submit.

### 6. Report to the user

```text
## Forall verification

- Job: vrf_...
- Status: succeeded | failed
- Summary: N proved / M property-tested / K spec-tracked

### CRITICAL
- ...

### WARNING
- ...

### Next
- Fix X in file Y, then re-run hosted verify
```

User-facing language: **Forall verified** / **machine-checked**.

## Guardrails

- Never claim success from an empty mapping / structure-only pass
- Never downgrade `verified: true` to silence failures — the flag is proof scope; earned outcomes live in the report's `ledger` (claimed vs earned levels, per-obligation prover identity) and hosted results now include it
- Prefer GitHub source when the revision is already public
- Keep API keys out of the repo and chat logs
- Cancel long jobs with `forall_cancel_verification` if the user aborts
