#!/usr/bin/env python3
"""Regression tests for the no-agent-attribution commit/PR guard."""

from pathlib import Path
import subprocess
import sys
import unittest


ROOT = Path(__file__).resolve().parents[2]
CHECKER = ROOT / "scripts" / "check_no_agent_attribution.py"


def check(text: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(CHECKER), "--stdin"],
        input=text,
        text=True,
        capture_output=True,
        check=False,
    )


class NoAgentAttributionTest(unittest.TestCase):
    def test_vendor_release_note_comparison_is_not_attribution(self) -> None:
        result = check(
            "The model approaches Claude Opus 4.8 on coding benchmarks."
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_standalone_model_signature_is_blocked(self) -> None:
        result = check("Claude Opus 4.8")
        self.assertEqual(result.returncode, 1)
        self.assertIn("agent model/product signature", result.stderr)

    def test_generated_with_tagline_is_blocked(self) -> None:
        result = check("Generated with Claude Code")
        self.assertEqual(result.returncode, 1)
        self.assertIn("Generated with <agent>", result.stderr)

    def test_reviewed_by_attribution_is_blocked(self) -> None:
        result = check("Reviewed by Claude Opus")
        self.assertEqual(result.returncode, 1)
        self.assertIn("<verb> by <agent>", result.stderr)


if __name__ == "__main__":
    unittest.main()
