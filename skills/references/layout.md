# `.forall` layout (hosted MCP authoring)

Create this layout when bootstrapping a brownfield project for hosted verify.
Paths are relative to the project root.

```text
.forall/
  AGENTS.md
  verify/
    mapping.yaml      # required marker
  workflow/
    config.yaml
  scenarios/          # optional *.property.ts
  specs/              # optional markdown specs
```

## `.forall/verify/mapping.yaml`

```yaml
version: 1
requirements: []
```

Replace `requirements` when mapping symbols (see `mapping.md`).

## `.forall/workflow/config.yaml`

```yaml
schema: forall
context: |
  This project uses Forall verification with hosted MCP.
  Author mapping and contracts locally; verify with forall_verify.
rules:
  verification:
    - Report proof status from the verify ledger, never from mapping flags
    - Prefer hosted forall_verify over skipping formal checks
```

## `.forall/AGENTS.md`

Short agent reminder (safe to customize):

```markdown
# Forall

Author `.forall/verify/mapping.yaml` and proof contracts in source.
Verify with hosted MCP (`forall_verify` → status → explain).
Say "Forall verified" / "machine-checked" in user-facing reports.
```

## Optional change workflow

If you want change-scoped checks without the CLI:

```text
.forall/workflow/changes/<kebab-name>/
  mapping.delta.yaml
  proposal.md          # optional
```

Hosted verify accepts `scope: { type: "change", name: "<kebab-name>" }`.

## Do not

- Leave mapping empty and call that “verified”
- Put secrets under `.forall/`
- Rely on the hosted worker to create these files in the user’s tree
