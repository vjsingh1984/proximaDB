"""TEMPORARY — TD-CI-2 acceptance probe. Removed in the next commit.

TD-CI-2's acceptance requires the widened gate to be OBSERVED failing on a
deliberately broken Python test, not assumed to work. This file is that probe:
it must turn the `Python SDK Tests` lane red on this PR, which is a lane that
would not have run at all before this change.
"""


def test_td_ci_2_probe_must_fail():
    assert 1 == 2, "TD-CI-2 acceptance probe: this failure is intentional"
