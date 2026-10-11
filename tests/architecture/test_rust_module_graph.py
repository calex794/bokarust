from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))

import rust_module_graph as graph  # noqa: E402


class RustModuleGraphTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.write("Cargo.toml", '[workspace]\nmembers = ["crates/site"]\nresolver = "2"\n')
        self.write("crates/site/Cargo.toml", '[package]\nname="site"\nversion="0.1.0"\nedition="2021"\n')

    def write(self, path: str, contents: str) -> None:
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(contents, encoding="utf-8")

    def analyze(self) -> graph.ModuleGraph:
        return graph.build_module_graph(self.root)

    def test_sibling_references_and_reverse_impact(self) -> None:
        self.write("crates/site/src/lib.rs", "pub mod books;\npub mod payments;\n")
        self.write("crates/site/src/books.rs", "use crate::payments::Receipt;\n")
        self.write("crates/site/src/payments.rs", "pub struct Receipt;\n")
        result = self.analyze()
        b = "crates/site/src/books.rs"
        p = "crates/site/src/payments.rs"
        self.assertEqual(result.dependencies[b], {p})
        self.assertEqual(result.dependents()[p], {b})
        self.assertEqual(result.impacted({p}), {p, b})
        self.assertEqual(result.issues, [])

    def test_grouped_uses_and_direct_qualified_references(self) -> None:
        self.write("crates/site/src/lib.rs", "mod books; mod payments; mod handler;\n")
        self.write("crates/site/src/books.rs", "pub struct Book;\n")
        self.write("crates/site/src/payments.rs", "pub struct Receipt;\n")
        self.write("crates/site/src/handler.rs", """use crate::{books::Book, payments::{self, Receipt}};
fn go() { let _ = crate::books::Book; }
""")
        result = self.analyze()
        self.assertEqual(result.dependencies["crates/site/src/handler.rs"], {
            "crates/site/src/books.rs", "crates/site/src/payments.rs"})

    def test_nested_inline_module_and_super_path(self) -> None:
        self.write("crates/site/src/lib.rs", """mod outer {
    mod reader;
    mod access { use super::reader::Reader; }
}
""")
        self.write("crates/site/src/outer/reader.rs", "pub struct Reader;\n")
        result = self.analyze()
        self.assertEqual(result.dependencies["crates/site/src/lib.rs"], {
            "crates/site/src/outer/reader.rs"})
        self.assertEqual(result.issues, [])

    def test_mod_rs_nesting(self) -> None:
        self.write("crates/site/src/lib.rs", "mod domain;\n")
        self.write("crates/site/src/domain/mod.rs", "mod books;\n")
        self.write("crates/site/src/domain/books.rs", "use super::Data;\n")
        result = self.analyze()
        self.assertEqual(result.dependencies["crates/site/src/domain/books.rs"], {
            "crates/site/src/domain/mod.rs"})

    def test_comments_strings_and_lifetimes_do_not_create_imports(self) -> None:
        self.write("crates/site/src/lib.rs", "mod books; mod payments;\n")
        self.write("crates/site/src/books.rs", """/* use crate::payments::Receipt; */
const TEXT: &str = r#"use crate::payments::Receipt;"#;
fn example<'a>(data: &'a str) -> &'a str { data }
""")
        self.write("crates/site/src/payments.rs", "pub struct Receipt;\n")
        result = self.analyze()
        self.assertEqual(result.dependencies["crates/site/src/books.rs"], set())

    def test_cycles_are_blocking_and_report_path(self) -> None:
        self.write("crates/site/src/lib.rs", "mod books; mod payments;\n")
        self.write("crates/site/src/books.rs", "use crate::payments::Receipt;\n")
        self.write("crates/site/src/payments.rs", "use crate::books::Book;\n")
        findings = graph.graph_findings(self.analyze(), {"limits": {}})
        cycles = [f for f in findings if f.rule == "RUST_MODULE_CYCLE"]
        self.assertEqual(len(cycles), 1)
        self.assertIn("crates/site/src/payments.rs", cycles[0].detail)
        self.assertEqual(cycles[0].severity, "BLOCK")

    def test_domain_owner_boundaries(self) -> None:
        self.write("crates/site/src/lib.rs", "mod books; mod money;\n")
        self.write("crates/site/src/books.rs", "use crate::money::Ledger;\n")
        self.write("crates/site/src/money.rs", "pub struct Ledger;\n")
        policy = {"module_owners": {
            "crates/site/src/books": "books",
            "crates/site/src/money": "finance"},
            "domain_dependencies": {"books": ["shared"]}}
        findings = graph.graph_findings(self.analyze(), policy)
        self.assertTrue(any(f.rule == "RUST_DOMAIN_BOUNDARY" and f.severity == "BLOCK"
                            for f in findings))
        policy["domain_dependencies"]["books"].append("finance")
        self.assertFalse(any(f.rule == "RUST_DOMAIN_BOUNDARY"
                             for f in graph.graph_findings(self.analyze(), policy)))

    def test_high_file_fanout_and_explicit_shared_exception(self) -> None:
        self.write("crates/site/src/lib.rs", "mod common; " +
                   " ".join(f"mod m{i};" for i in range(10)))
        self.write("crates/site/src/common.rs", "pub struct Id;\n")
        for i in range(10):
            self.write(f"crates/site/src/m{i}.rs", "use crate::common::Id;\n")
        report = self.analyze()
        policy = {"limits": {"module_fanout_warn": 8, "module_fanout_block": 15}}
        warnings = [f for f in graph.graph_findings(report, policy) if f.rule == "RUST_MODULE_FANOUT"]
        self.assertEqual(len(warnings), 1)
        self.assertEqual(warnings[0].severity, "WARN")
        policy["shared_modules"] = ["crates/site/src/common"]
        self.assertFalse(any(f.rule == "RUST_MODULE_FANOUT"
                             for f in graph.graph_findings(report, policy)))

    def test_fanout_above_hard_limit_blocks(self) -> None:
        self.write("crates/site/src/lib.rs", "mod common; " +
                   " ".join(f"mod m{i};" for i in range(16)))
        self.write("crates/site/src/common.rs", "pub struct Id;\n")
        for i in range(16):
            self.write(f"crates/site/src/m{i}.rs", "use crate::common::Id;\n")
        findings = graph.graph_findings(self.analyze(), {"limits": {}})
        self.assertTrue(any(f.rule == "RUST_MODULE_FANOUT" and f.severity == "BLOCK"
                            for f in findings))

    def test_broken_module_is_never_silently_ignored(self) -> None:
        self.write("crates/site/src/lib.rs", "mod missing;\n")
        result = self.analyze()
        self.assertTrue(any(f.rule == "RUST_MODULE_UNRESOLVED" for f in result.issues))

    def test_include_macro_is_reported_as_incomplete_scan(self) -> None:
        self.write("crates/site/src/lib.rs", 'include!("generated.rs");\n')
        result = self.analyze()
        self.assertTrue(any(f.rule == "RUST_MACRO_IMPORT_UNKNOWN" for f in result.issues))


if __name__ == "__main__":
    unittest.main()
