from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))

import select_rust_packages as selector  # noqa: E402


class RustTestSelectionTests(unittest.TestCase):
    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.write("Cargo.toml", """[workspace]
members = ["crates/*"]
resolver = "2"
[workspace.dependencies]
book-domain = { path = "crates/domain" }
""")
        self.crate("domain", "")
        self.crate("application", """[dependencies]
book-domain = { workspace = true }
""")
        self.crate("adapters", """[dependencies]
app = { package = "book-application", path = "../application" }
book-domain = { path = "../domain" }
""")
        self.crate("web", """[dependencies]
adapter-layer = { package = "book-adapters", path = "../adapters" }
book-application = { path = "../application" }
""")

    def write(self, path: str, content: str) -> None:
        file = self.root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(content, encoding="utf-8")

    def crate(self, suffix: str, extra: str) -> None:
        self.write(f"crates/{suffix}/Cargo.toml",
                   "[package]\n" +
                   f'name = "book-{suffix}"\nversion = "0.1.0"\n' + extra)
        self.write(f"crates/{suffix}/src/lib.rs", "pub fn test() {}\n")

    def selection(self, *paths: str) -> selector.Selection:
        return selector.select(self.root, list(paths))

    def test_leaf_web_change_does_not_trigger_other_crates(self) -> None:
        result = self.selection("crates/web/src/lib.rs")
        self.assertEqual(result.selected_packages, ("book-web",))
        self.assertFalse(result.full)

    def test_adapter_change_retests_its_web_consumer(self) -> None:
        result = self.selection("crates/adapters/src/repository.rs")
        self.assertEqual(result.selected_packages, ("book-adapters", "book-web"))

    def test_application_change_retests_all_dependents(self) -> None:
        result = self.selection("crates/application/src/usecase.rs")
        self.assertEqual(result.selected_packages, (
            "book-adapters", "book-application", "book-web"))

    def test_domain_change_includes_all_transitive_consumers(self) -> None:
        result = self.selection("crates/domain/src/books.rs")
        self.assertEqual(result.selected_packages, (
            "book-adapters", "book-application", "book-domain", "book-web"))

    def test_deleted_crate_file_still_selects_its_package(self) -> None:
        result = self.selection("crates/web/src/deleted.rs")
        self.assertEqual(result.selected_packages, ("book-web",))

    def test_workspace_build_changes_trigger_full(self) -> None:
        for path in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml",
                     ".config/nextest.toml", ".github/workflows/workspace.yml"):
            with self.subTest(path=path):
                self.assertTrue(self.selection(path).full)

    def test_unknown_rust_file_triggers_full(self) -> None:
        result = self.selection("another-crate/src/lib.rs")
        self.assertTrue(result.full)
        self.assertEqual(len(result.selected_packages), 4)

    def test_documentation_and_architecture_only_skip_rust_tests(self) -> None:
        result = self.selection("README.md", "docs/architecture/spec.md",
                                "architecture/policy.toml",
                                "tests/architecture/test_architecture_check.py")
        self.assertFalse(result.full)
        self.assertEqual(result.selected_packages, ())

    def test_mixed_docs_and_rust_change_keeps_rust_tests(self) -> None:
        result = self.selection("README.md", "crates/web/src/routes.rs")
        self.assertEqual(result.selected_packages, ("book-web",))

    def test_selector_changes_force_full_run(self) -> None:
        result = self.selection("tools/select_rust_packages.py")
        self.assertTrue(result.full)

    def test_empty_diff_runs_full_not_fake_green(self) -> None:
        self.assertTrue(self.selection().full)

    def test_explicit_full_requested(self) -> None:
        result = selector.select(self.root, [], force_all=True)
        self.assertEqual(len(result.selected_packages), 4)
        self.assertTrue(result.full)

    def test_invalid_local_dependency_is_an_error(self) -> None:
        self.crate("web", """[dependencies]
external = { path = "../missing" }
""")
        with self.assertRaisesRegex(ValueError, "outside workspace"):
            selector.workspace(self.root)

    def test_dev_dependencies_also_have_reverse_impact(self) -> None:
        self.crate("web", """[dev-dependencies]
book-domain = { path = "../domain" }
""")
        result = self.selection("crates/domain/src/books.rs")
        self.assertIn("book-web", result.selected_packages)

    def test_unowned_path_cannot_disable_tests(self) -> None:
        result = self.selection("../../outside.rs")
        self.assertTrue(result.full)


if __name__ == "__main__":
    unittest.main()
