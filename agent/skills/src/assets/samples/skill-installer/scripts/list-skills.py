#!/usr/bin/env python3
"""List skills from $FORALL_HOME/skills or a GitHub repo path."""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error

from github_utils import github_api_contents_url, github_request

DEFAULT_PATH = "skills"
DEFAULT_REF = "main"
SYSTEM_SKILLS_DIR = ".system"


class ListError(Exception):
    pass


class Args(argparse.Namespace):
    local: bool
    repo: str | None
    path: str
    ref: str
    format: str


def _request(url: str) -> bytes:
    return github_request(url, "forall-skill-list")


def _forall_home() -> str:
    return os.environ.get("FORALL_HOME", os.path.expanduser("~/.forall"))


def _installed_skills() -> dict[str, bool]:
    root = os.path.join(_forall_home(), "skills")
    if not os.path.isdir(root):
        return {}
    entries: dict[str, bool] = {}
    for name in os.listdir(root):
        path = os.path.join(root, name)
        if not os.path.isdir(path):
            continue
        if name == SYSTEM_SKILLS_DIR:
            system_root = path
            if not os.path.isdir(system_root):
                continue
            for skill_name in os.listdir(system_root):
                skill_path = os.path.join(system_root, skill_name)
                if os.path.isdir(skill_path):
                    entries[skill_name] = True
            continue
        entries[name] = False
    return entries


def _list_local_skills() -> list[tuple[str, bool]]:
    installed = _installed_skills()
    return sorted(installed.items(), key=lambda item: item[0])


def _list_skills(repo: str, path: str, ref: str) -> list[str]:
    api_url = github_api_contents_url(repo, path, ref)
    try:
        payload = _request(api_url)
    except urllib.error.HTTPError as exc:
        if exc.code == 404:
            raise ListError(
                "Skills path not found: "
                f"https://github.com/{repo}/tree/{ref}/{path}"
            ) from exc
        raise ListError(f"Failed to fetch skills: HTTP {exc.code}") from exc
    data = json.loads(payload.decode("utf-8"))
    if not isinstance(data, list):
        raise ListError("Unexpected skills listing response.")
    skills = [item["name"] for item in data if item.get("type") == "dir"]
    return sorted(skills)


def _parse_args(argv: list[str]) -> Args:
    parser = argparse.ArgumentParser(description="List skills.")
    parser.add_argument(
        "--local",
        action="store_true",
        help="List skills installed under $FORALL_HOME/skills (default when --repo is omitted)",
    )
    parser.add_argument(
        "--repo",
        help="GitHub repo in owner/repo form (required for remote listing)",
    )
    parser.add_argument(
        "--path",
        default=DEFAULT_PATH,
        help="Repo path to list (default: skills)",
    )
    parser.add_argument("--ref", default=DEFAULT_REF)
    parser.add_argument(
        "--format",
        choices=["text", "json"],
        default="text",
        help="Output format",
    )
    return parser.parse_args(argv, namespace=Args())


def main(argv: list[str]) -> int:
    args = _parse_args(argv)
    use_local = args.local or not args.repo
    try:
        if use_local:
            skills = _list_local_skills()
            if args.format == "json":
                payload = [
                    {
                        "name": name,
                        "bundled": bundled,
                        "installed": True,
                    }
                    for name, bundled in skills
                ]
                print(json.dumps(payload))
            else:
                for idx, (name, bundled) in enumerate(skills, start=1):
                    suffix = " (bundled)" if bundled else ""
                    print(f"{idx}. {name}{suffix}")
            return 0

        if not args.repo:
            raise ListError("Remote listing requires --repo <owner>/<repo>.")

        remote_skills = _list_skills(args.repo, args.path, args.ref)
        installed = set(_installed_skills())
        if args.format == "json":
            payload = [
                {"name": name, "installed": name in installed} for name in remote_skills
            ]
            print(json.dumps(payload))
        else:
            for idx, name in enumerate(remote_skills, start=1):
                suffix = " (already installed)" if name in installed else ""
                print(f"{idx}. {name}{suffix}")
        return 0
    except ListError as exc:
        print(f"Error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
