from __future__ import annotations

import tomllib
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


class NextestProfileTest(unittest.TestCase):
    def test_ci_and_nightly_have_finite_budgets(self) -> None:
        path = ROOT / ".config/nextest.toml"
        with path.open("rb") as source:
            config = tomllib.load(source)
        profiles = config["profile"]
        default = profiles["default"]
        self.assertEqual(default["retries"], 0)
        self.assertTrue(default["fail-fast"])
        self.assertEqual(default["global-timeout"], "10m")
        self.assertEqual(default["slow-timeout"]["terminate-after"], 1)
        self.assertEqual(default["slow-timeout"]["on-timeout"], "fail")

        self.assertEqual(profiles["ci"]["inherits"], "default")
        overrides = profiles["ci"]["overrides"]
        self.assertEqual(len(overrides), 1)
        self.assertEqual(overrides[0]["slow-timeout"]["terminate-after"], 1)
        self.assertEqual(profiles["nightly"]["global-timeout"], "30m")
        self.assertEqual(profiles["nightly"]["slow-timeout"]["terminate-after"], 1)


if __name__ == "__main__":
    unittest.main()
