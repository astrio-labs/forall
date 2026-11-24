#!/usr/bin/env python3
"""Invoked by forall-workflow property-tests phase for `.property.py` files."""
from __future__ import annotations

import importlib.util
import inspect
import json
import os
import sys
from pathlib import Path

# Import failures that mean the property-testing tool is missing rather than
# the property being wrong. Keep in sync with the generator libraries Forall
# writes into `.property.py` templates.
PROPERTY_LIBRARIES = frozenset({"hypothesis"})


def _raised_inside_property_library(err: BaseException) -> bool:
    """Whether the failed import happened inside a generator library.

    A library importing one of its own missing dependencies raises with that
    dependency's name, not the library's, so the name alone would call a
    half-installed Hypothesis a failed property. The traceback says where the
    import was actually attempted.
    """
    frame = err.__traceback__
    while frame is not None:
        module = frame.tb_frame.f_globals.get("__name__", "")
        if module.split(".")[0] in PROPERTY_LIBRARIES:
            return True
        frame = frame.tb_next
    return False


def main() -> int:
    if len(sys.argv) < 2:
        print(json.dumps({"ok": False, "message": "missing property file path"}))
        return 1

    # The property imports the code under test, usually as `from src.foo
    # import bar`. Loading a module by file path does not put the project on
    # sys.path, and the runner lives outside the project, so without this a
    # property cannot import the very code it exists to check. Forall runs the
    # runner with the project root as cwd.
    project_root = os.getcwd()
    if project_root not in sys.path:
        sys.path.insert(0, project_root)

    path = Path(sys.argv[1]).resolve()
    spec = importlib.util.spec_from_file_location("forall_property", path)
    if spec is None or spec.loader is None:
        print(json.dumps({"ok": False, "message": f"could not load {path}"}))
        return 1

    # Loading is separated from running on purpose. A property module that
    # cannot import its generator library has not falsified anything — the
    # check never happened — and reporting that as a failed property writes a
    # false statement into the ledger.
    #
    # Only the generator library's absence counts. Python's import system
    # cannot tell a third-party package from the project's own module — a
    # ModuleNotFoundError carries a bare name and nothing else — so the
    # libraries are named here rather than inferred. The alternative, treating
    # every missing import as a tooling gap, turns a typo in the import of the
    # code under test into a warning, and a property that can never run would
    # then sit quietly under a passing check forever. Anything not named below
    # is a defect in the author's own file and keeps failing.
    module = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(module)
    except ModuleNotFoundError as err:
        missing = (err.name or "").split(".")[0]
        tooling_gap = missing in PROPERTY_LIBRARIES or _raised_inside_property_library(
            err
        )
        print(
            json.dumps(
                {"ok": False, "toolUnavailable": tooling_gap, "message": str(err)}
            )
        )
        return 1
    except Exception as err:  # noqa: BLE001 — the author's file is broken
        print(json.dumps({"ok": False, "message": str(err)}))
        return 1

    run = getattr(module, "default", None)
    if run is None:
        run = getattr(module, "runPropertyTests", None)
    if not callable(run):
        print(
            json.dumps(
                {
                    "ok": False,
                    "message": "property module must export runPropertyTests() or default",
                }
            )
        )
        return 1

    try:
        if inspect.iscoroutinefunction(run):
            import asyncio

            result = asyncio.run(run())
        else:
            result = run()
    except Exception as err:  # noqa: BLE001 — report structured failure to stdout
        print(json.dumps({"ok": False, "message": str(err)}))
        return 1

    print(json.dumps(result if result is not None else {"ok": False}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
