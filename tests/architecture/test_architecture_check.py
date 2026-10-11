from __future__ import annotations

import sys
import tempfile
import unittest
from datetime import date, timedelta
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))

import architecture_check as checker  # noqa: E402


POLICY = {
    "limits": {
        "source_warn": 400,
        "source_block": 600,
        "test_warn": 800,
        "test_block": 1000,
        "fanout_warn": 8,
        "fanout_block": 15,
    },
    "packages": {"book-domain": "domain", "web": "edge", "db": "infrastructure"},
    "forbidden_external": {"domain": ["axum", "sqlx", "reqwest"]},
}


class ArchitectureChecksTest(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def write(self, relative: str, text: str) -> None:
        file = self.root / relative
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text, encoding="utf-8")

    def write_workspace(self, extra: str = "", domain_dependencies: str = "") -> None:
        self.write("Cargo.toml", '[workspace]\nmembers = ["crates/*"]\nresolver = "2"\n')
        self.write("crates/book-domain/Cargo.toml",
                   '[package]\nname = "book-domain"\nversion = "0.1.0"\n'
                   '[dependencies]\n' + domain_dependencies)
        self.write("crates/web/Cargo.toml",
                   '[package]\nname = "web"\nversion = "0.1.0"\n'
                   '[dependencies]\nbook-domain = { path = "../book-domain" }\n'
                   + extra)

    def codes(self, findings: list[checker.Finding]) -> set[str]:
        return {item.rule for item in findings if item.severity == "BLOCK"}

    def test_file_limit_and_warning(self) -> None:
        self.write("crates/book-domain/src/lib.rs", "fn sample() {}\n" * 601)
        result = checker.file_findings(self.root, ["crates/book-domain/src/lib.rs"], POLICY)
        self.assertIn("FILE_SIZE", self.codes(result))
        self.write("crates/book-domain/src/lib.rs", "fn sample() {}\n" * 401)
        result = checker.file_findings(self.root, ["crates/book-domain/src/lib.rs"], POLICY)
        self.assertEqual([item.severity for item in result], ["WARN"])

    def test_test_files_have_their_own_limit(self) -> None:
        self.write("crates/book-domain/tests/example.rs", "// test\n" * 900)
        result = checker.file_findings(self.root, ["crates/book-domain/tests/example.rs"], POLICY)
        self.assertEqual([item.severity for item in result], ["WARN"])

    def test_regular_workspace_and_dependency_direction(self) -> None:
        self.write_workspace()
        findings, graph = checker.workspace_findings(self.root, POLICY)
        self.assertEqual(self.codes(findings), set())
        self.assertEqual(graph["web"], {"book-domain"})

    def test_domain_cannot_import_web_or_database_client(self) -> None:
        self.write_workspace(domain_dependencies='web = { path = "../web" }\nsqlx = "0.8"\n')
        findings, _ = checker.workspace_findings(self.root, POLICY)
        self.assertIn("LAYER_DIRECTION", self.codes(findings))
        self.assertIn("DOMAIN_IO", self.codes(findings))
        self.assertIn("CYCLE", self.codes(findings))

    def test_unknown_package_is_blocked(self) -> None:
        self.write_workspace()
        policy = {**POLICY, "packages": {"book-domain": "domain"}}
        findings, _ = checker.workspace_findings(self.root, policy)
        self.assertIn("UNOWNED_PACKAGE", self.codes(findings))

    def test_direct_fanout_warns_but_shared_is_exempt(self) -> None:
        graph = {"core": set()}
        for n in range(10):
            graph[f"p{n}"] = {"core"}
        self.assertEqual(checker._cycles(graph), [])
        # The package fanout report is based on the same reverse edge mapping.
        self.write("Cargo.toml", '[workspace]\nmembers = ["crates/*"]\n')
        self.write("crates/core/Cargo.toml", '[package]\nname="core"\nversion="0.1.0"\n')
        for n in range(10):
            self.write(f"crates/p{n}/Cargo.toml",
                       f'[package]\nname="p{n}"\nversion="0.1.0"\n'
                       '[dependencies]\ncore = { path = "../core" }\n')
        policy = {**POLICY, "packages": {"core": "domain", **{
            f"p{n}": "edge" for n in range(10)}}}
        findings, _ = checker.workspace_findings(self.root, policy)
        self.assertTrue(any(f.rule == "FANOUT" and f.severity == "WARN" for f in findings))
        policy["packages"]["core"] = "shared"
        findings, _ = checker.workspace_findings(self.root, policy)
        self.assertFalse(any(f.rule == "FANOUT" for f in findings))

    def test_cycles_explain_the_path(self) -> None:
        graph = {"books": {"commerce"}, "commerce": {"books"}, "other": set()}
        self.assertEqual(checker._cycles(graph), [["books", "commerce", "books"]])

    def test_workflows_fail_without_finite_timeouts(self) -> None:
        self.write(".github/workflows/ci.yml",
                   "name: CI\njobs:\n  check:\n    runs-on: ubuntu-latest\n")
        self.assertIn("JOB_TIMEOUT", self.codes(checker.workflow_findings(self.root)))
        self.write(".github/workflows/ci.yml",
                   "name: CI\njobs:\n  check:\n    runs-on: ubuntu-latest\n"
                   "    timeout-minutes: 10\n")
        self.assertEqual(self.codes(checker.workflow_findings(self.root)), set())

    def test_exact_exception_requires_reason_and_expiry(self) -> None:
        offending = checker.Finding("BLOCK", "FILE_SIZE", "src/lib.rs", "700 lines")
        deadline = date.today() + timedelta(days=7)
        allowed = checker.exception_findings([offending], {
            "exceptions": [{"rule": "FILE_SIZE", "path": "src/lib.rs",
                            "reason": "Generated bridge awaiting refactor", "issue": "BKR-12",
                            "expires": deadline}]
        })
        self.assertEqual([f.severity for f in allowed], ["WARN"])
        expired = checker.exception_findings([offending], {
            "exceptions": [{"rule": "FILE_SIZE", "path": "src/lib.rs",
                            "reason": "Old", "issue": "BKR-12",
                            "expires": date.today() - timedelta(days=1)}]
        })
        self.assertIn("INVALID_EXCEPTION", self.codes(expired))

    def test_stale_exception_is_an_error(self) -> None:
        items = checker.exception_findings([], {
            "exceptions": [{"rule": "FILE_SIZE", "path": "src/lib.rs",
                            "reason": "No longer used", "issue": "BKR-12",
                            "expires": date.today() + timedelta(days=7)}]
        })
        self.assertIn("STALE_EXCEPTION", self.codes(items))

    def test_invalid_path_cannot_escape_repository(self) -> None:
        items = checker.file_findings(self.root, ["../../outside.rs"], POLICY)
        self.assertIn("INVALID_PATH", self.codes(items))


if __name__ == "__main__":
    unittest.main()
