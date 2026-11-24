---
name: skill-installer
description: Install Forall skills into $FORALL_HOME/skills from a GitHub repo path. Use when a user asks to list installable skills from a repo, install a skill from GitHub (including private repos), or inspect which skills are already installed locally.
metadata:
  short-description: Install skills from GitHub into $FORALL_HOME/skills
---

# Skill Installer

Helps install skills from GitHub into `$FORALL_HOME/skills` (defaults to `~/.forall/skills`).

Bundled system skills ship with Forall under `$FORALL_HOME/skills/.system/` (for example `skill-creator`, `plugin-creator`, `forall-docs`, and `skill-installer`). Do not reinstall those unless the user explicitly asks to overwrite them.

Use the helper scripts based on the task:
- List locally installed skills (including bundled `.system` skills) when the user asks what is available without naming a GitHub repo.
- List skills from a GitHub repo path when the user names a repo or wants a remote catalog.
- Install from a GitHub repo when the user provides a repo/path (including private repos).

Install skills with the helper scripts.

## Communication

When listing local skills, output approximately as follows:
"""
Installed skills in $FORALL_HOME/skills:
1. skill-creator (bundled)
2. my-custom-skill
3. ...
Which GitHub skill(s) would you like installed?
"""

When listing remote skills from a repo, output approximately as follows:
"""
Skills from {repo}/{path}:
1. skill-1
2. skill-2 (already installed)
3. ...
Which ones would you like installed?
"""

After installing a skill, tell the user: "Restart Forall to pick up new skills."

## Scripts

All of these scripts use network when listing or installing from GitHub, so when running in the sandbox, request escalation when running them.

- `scripts/list-skills.py --local` (lists installed skills under `$FORALL_HOME/skills`)
- `scripts/list-skills.py --local --format json`
- `scripts/list-skills.py --repo <owner>/<repo> --path <path/to/skills>` (remote catalog)
- `scripts/list-skills.py --repo <owner>/<repo> --path <path> --format json`
- `scripts/install-skill-from-github.py --repo <owner>/<repo> --path <path/to/skill> [<path/to/skill> ...]`
- `scripts/install-skill-from-github.py --url https://github.com/<owner>/<repo>/tree/<ref>/<path>`

## Behavior and Options

- Defaults to direct download for public GitHub repos.
- If download fails with auth/permission errors, falls back to git sparse checkout.
- Aborts if the destination skill directory already exists.
- Installs into `$FORALL_HOME/skills/<skill-name>` (defaults to `~/.forall/skills`).
- Multiple `--path` values install multiple skills in one run, each named from the path basename unless `--name` is supplied.
- Options: `--ref <ref>` (default `main`), `--dest <path>`, `--method auto|download|git`.

## Notes

- Remote listing requires `--repo` and `--path`. There is no default upstream catalog.
- Private GitHub repos can be accessed via existing git credentials or optional `GITHUB_TOKEN`/`GH_TOKEN` for download.
- Git fallback tries HTTPS first, then SSH.
- Bundled skills under `$FORALL_HOME/skills/.system/` are managed by Forall. If the user asks to reinstall one, explain that it is bundled first; only download and overwrite if they insist.
- Installed annotations come from `$FORALL_HOME/skills`.
