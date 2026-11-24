"""Example Python property test (Hypothesis).

Copy to .forall/scenarios/<requirement-id>.property.py and adjust the import.
Requires Hypothesis in the project:  pip install hypothesis

Forall runs this file through scripts/run-property.py, which imports it and
calls `default` (or `runPropertyTests`). Return a dict:

    {"ok": bool, "counterexample": any, "seed": int, "examplesRun": int}
"""

from __future__ import annotations

import os

from hypothesis import HealthCheck, given, seed, settings
from hypothesis import strategies as st

from src.clamp import clamp  # adjust to your module


def default() -> dict:
    # Forall passes these so a counterexample can be reproduced. Honour them:
    # the ledger records the seed alongside the result, and a run that ignored
    # it would record a seed that reproduces nothing.
    pbt_seed = int(os.environ.get("FORALL_PBT_SEED", "0"))
    max_examples = int(os.environ.get("FORALL_PBT_EXAMPLES", "100"))

    # @seed pins the run. It sets database=None itself, so a previously
    # recorded failure cannot replay and make the result depend on local state.
    @seed(pbt_seed)
    @settings(
        max_examples=max_examples,
        # Properties here assert rather than return, but a slow example should
        # not be reported as a falsification.
        suppress_health_check=[HealthCheck.too_slow],
    )
    @given(st.integers(), st.integers(), st.integers())
    def check(x: int, lo: int, hi: int) -> None:
        # Precondition, the counterpart of a `requires` clause.
        if lo > hi:
            return
        result = clamp(x, lo, hi)
        # Assert. Do NOT `return True` — Hypothesis treats a returned value as
        # a mistake and raises FailedHealthCheck, so a property ported from a
        # fast-check example that returns a boolean will fail rather than pass.
        assert lo <= result <= hi

    try:
        check()
    except BaseException as err:  # noqa: BLE001
        # Hypothesis re-raises the property's OWN exception rather than a
        # wrapper type, and its errors module has no falsification class to
        # catch. StopTest is BaseException-but-not-Exception, so catching
        # Exception would let it escape.
        notes = getattr(err, "__notes__", None)
        return {
            "ok": False,
            "counterexample": notes[0] if notes else str(err),
            "seed": pbt_seed,
            "examplesRun": max_examples,
        }

    # Hypothesis stops generating at the first failure and then shrinks, so the
    # number of times the body ran is not the number of examples explored.
    # Report the configured count rather than a raw invocation tally.
    return {"ok": True, "seed": pbt_seed, "examplesRun": max_examples}
