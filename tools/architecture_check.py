#!/usr/bin/env python3
"""Fast, dependency-free repository architecture checks.

The parser is deliberately conservative: package-level imports are enforced,
while incomplete file-level Rust import analysis is not presented as proof.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from collections import defaultdict
from dataclasses import dataclass
from datetime import date
from pathlib import Path

from rust_module_graph import build_module_graph, graph_findings


SOURCE_SUFFIXES = {".rs", ".js", ".ts", ".css"}
WORKFLOW_SUFFIXES = {".yml", ".yaml"}
ALLOWED_KINDS = {"shared", "domain", "application", "infrastructure", "edge"}
DEPENDENCY_POLICY = {
    "shared": {"shared"},
    "domain": {"shared", "domain"},
    "application": {"shared", "domain", "application"},
    "infrastructure": {"shared", "domain", "application", "infrastructure"},
    "edge": ALLOWED_KINDS,
}


@dataclass(frozen=True)
class Finding:
    severity: str
    rule: str
    path: str
    detail: str

    def render(self) -> str:
        return f"{self.severity} {self.rule} {self.path}: {self.detail}"


def read_policy(root: Path) -> dict:
    path = root / "architecture/policy.toml"
    with path.open("rb") as handle:
        return tomllib.load(handle)


def validate_policy(policy: dict) -> list[Finding]:
    issues: list[Finding] = []
    limits = policy.get("limits", {})
    for prefix in ("source", "test"):
        warn, block = limits.get(f"{prefix}_warn"), limits.get(f"{prefix}_block")
        if not isinstance(warn, int) or not isinstance(block, int) or not 0 < warn < block:
            issues.append(Finding("BLOCK", "POLICY", "architecture/policy.toml",
                                  f"invalid {prefix} limits: {warn!r}/{block!r}"))
    for prefix in ("fanout", "module_fanout"):
        warn, block = limits.get(f"{prefix}_warn"), limits.get(f"{prefix}_block")
        if not isinstance(warn, int) or not isinstance(block, int) or not 0 < warn < block:
            issues.append(Finding("BLOCK", "POLICY", "architecture/policy.toml",
                                  f"invalid {prefix} limits: {warn!r}/{block!r}"))
    for path in policy.get("shared_modules", []):
        if (not isinstance(path, str) or not path.startswith("crates/")
                or "/src/" not in path or "*" in path):
            issues.append(Finding("BLOCK", "POLICY", "architecture/policy.toml",
                                  f"shared module path is too broad: {path!r}"))
    for name, kind in policy.get("packages", {}).items():
        if kind not in ALLOWED_KINDS:
            issues.append(Finding("BLOCK", "POLICY", name, f"unknown package kind {kind!r}"))
    return issues


def git_paths(root: Path, base: str | None) -> list[str]:
    if base:
        command = ["git", "diff", "--name-only", "--diff-filter=ACMR", "-z",
                   f"{base}...HEAD"]
    else:
        command = ["git", "ls-files", "-z"]
    result = subprocess.run(command, cwd=root, check=False, capture_output=True,
                            timeout=30)
    if result.returncode:
        raise RuntimeError(f"{' '.join(command)} failed: "
                           f"{result.stderr.decode(errors='replace').strip()}")
    return [p.decode("utf-8", errors="surrogateescape")
            for p in result.stdout.split(b"\0") if p]


def file_findings(root: Path, paths: list[str], policy: dict) -> list[Finding]:
    limits = policy["limits"]
    findings: list[Finding] = []
    for raw in sorted(set(paths)):
        relative = Path(raw)
        if relative.is_absolute() or ".." in relative.parts:
            findings.append(Finding("BLOCK", "INVALID_PATH", raw, "path escapes repository"))
            continue
        if relative.suffix.lower() not in SOURCE_SUFFIXES:
            continue
        path = root / relative
        if not path.is_file():
            continue  # Deleted files are handled by Git, not by file sizing.
        try:
            lines = len(path.read_text(encoding="utf-8").splitlines())
        except (UnicodeError, OSError) as exc:
            findings.append(Finding("BLOCK", "UNREADABLE_SOURCE", raw, str(exc)))
            continue
        prefix = "test" if "tests" in relative.parts or relative.stem.startswith("test_") else "source"
        warn, block = limits[f"{prefix}_warn"], limits[f"{prefix}_block"]
        if lines > block:
            findings.append(Finding("BLOCK", "FILE_SIZE", raw,
                                    f"{lines} lines; limit {block}"))
        elif lines > warn:
            findings.append(Finding("WARN", "FILE_SIZE", raw,
                                    f"{lines} lines; recommended at most {warn}"))
    return findings


def workflow_findings(root: Path) -> list[Finding]:
    """Require a finite job timeout using ordinary, unanchored GitHub workflow form."""
    directory = root / ".github/workflows"
    if not directory.exists():
        return []
    findings: list[Finding] = []
    for path in sorted(directory.iterdir()):
        if path.suffix not in WORKFLOW_SUFFIXES or not path.is_file():
            continue
        # Nightly full audits may run longer than PR checks, but are still finite.
        contents = path.read_text(encoding="utf-8")
        nightly_only = (path.name == "rust-full-audit.yml"
                        and re.search(r"(?m)^  schedule:\\s*$", contents)
                        and not re.search(r"(?m)^  pull_request:\\s*$", contents))
        max_minutes = 45 if nightly_only else 30
        jobs_started = False
        active_job: str | None = None
        active_timeout: int | None = None

        def flush() -> None:
            if active_job is not None and active_timeout is None:
                findings.append(Finding("BLOCK", "JOB_TIMEOUT",
                                        path.relative_to(root).as_posix(),
                                        f"job {active_job!r} lacks timeout-minutes"))

        for line in contents.splitlines():
            if line == "jobs:":
                jobs_started = True
                continue
            if not jobs_started:
                continue
            job = re.match(r"^  ([A-Za-z][\w-]*):\s*(?:#.*)?$", line)
            if job:
                flush()
                active_job, active_timeout = job.group(1), None
                continue
            if active_job:
                timeout = re.match(r"^    timeout-minutes:\s*(\d+)\s*(?:#.*)?$", line)
                if timeout:
                    active_timeout = int(timeout.group(1))
                    if not 1 <= active_timeout <= max_minutes:
                        findings.append(Finding("BLOCK", "JOB_TIMEOUT",
                                                path.relative_to(root).as_posix(),
                                                f"job {active_job!r}: invalid "
                                                f"{active_timeout} minutes"))
        flush()
    return findings


def _dependency_specs(data: dict) -> dict:
    deps = dict(data.get("dependencies", {}))
    deps.update(data.get("build-dependencies", {}))
    for target in data.get("target", {}).values():
        deps.update(target.get("dependencies", {}))
        deps.update(target.get("build-dependencies", {}))
    return deps


def _cycles(graph: dict[str, set[str]]) -> list[list[str]]:
    """Report cycles with a path rather than a list of unordered members."""
    visited: set[str] = set()
    active: list[str] = []
    result: list[list[str]] = []
    found: set[tuple[str, ...]] = set()

    def walk(current: str) -> None:
        visited.add(current)
        active.append(current)
        for next_name in sorted(graph[current]):
            if next_name in active:
                sequence = active[active.index(next_name):] + [next_name]
                key = tuple(sorted(set(sequence)))
                if key not in found:
                    found.add(key)
                    result.append(sequence)
            elif next_name not in visited:
                walk(next_name)
        active.pop()

    for package in sorted(graph):
        if package not in visited:
            walk(package)
    return result


def workspace_findings(root: Path, policy: dict) -> tuple[list[Finding], dict[str, set[str]]]:
    manifest = root / "Cargo.toml"
    if not manifest.is_file():
        return [], {}  # The base repository is still in its research phase.
    findings: list[Finding] = []
    try:
        with manifest.open("rb") as handle:
            workspace_data = tomllib.load(handle)
        workspace = workspace_data.get("workspace", {})
        members = workspace.get("members", [])
        manifests: list[Path] = [manifest] if "package" in workspace_data else []
        for pattern in members:
            manifests.extend(root.glob(f"{pattern}/Cargo.toml"))
        manifests = sorted(set(manifests))
        if not manifests:
            return [Finding("BLOCK", "WORKSPACE", "Cargo.toml",
                            "no package manifests discovered")], {}
        data_by_name: dict[str, tuple[Path, dict]] = {}
        directory_to_name: dict[Path, str] = {}
        for path in manifests:
            with path.open("rb") as handle:
                data = tomllib.load(handle)
            name = data["package"]["name"]
            if name in data_by_name:
                raise ValueError(f"duplicate Cargo package {name}")
            data_by_name[name] = path, data
            directory_to_name[path.parent.resolve()] = name
    except (OSError, KeyError, TypeError, ValueError, tomllib.TOMLDecodeError) as exc:
        return [Finding("BLOCK", "WORKSPACE", "Cargo.toml", str(exc))], {}

    kinds = policy.get("packages", {})
    forbidden = set(policy.get("forbidden_external", {}).get("domain", []))
    graph: dict[str, set[str]] = {name: set() for name in data_by_name}
    for name, (path, data) in data_by_name.items():
        kind = kinds.get(name)
        if kind not in ALLOWED_KINDS:
            findings.append(Finding("BLOCK", "UNOWNED_PACKAGE",
                                    path.relative_to(root).as_posix(),
                                    f"{name} must have a layer in policy.toml"))
        for dep_name, spec in _dependency_specs(data).items():
            if isinstance(spec, dict) and spec.get("workspace") is True:
                spec = workspace_data.get("workspace", {}).get("dependencies", {}).get(dep_name, {})
            if kind == "domain" and (dep_name in forbidden or
                                      isinstance(spec, dict) and spec.get("package") in forbidden):
                findings.append(Finding("BLOCK", "DOMAIN_IO",
                                        path.relative_to(root).as_posix(),
                                        f"domain package {name} depends on {dep_name}"))
            if not isinstance(spec, dict) or "path" not in spec:
                continue
            location = (path.parent / spec["path"]).resolve()
            actual_name = directory_to_name.get(location)
            if not actual_name:
                findings.append(Finding("BLOCK", "UNRESOLVED_LOCAL_DEP",
                                        path.relative_to(root).as_posix(),
                                        f"{dep_name} refers to unknown local path {location}"))
                continue
            graph[name].add(actual_name)
            dependency_kind = kinds.get(actual_name)
            if kind in DEPENDENCY_POLICY and dependency_kind in ALLOWED_KINDS:
                if dependency_kind not in DEPENDENCY_POLICY[kind]:
                    findings.append(Finding("BLOCK", "LAYER_DIRECTION",
                                            path.relative_to(root).as_posix(),
                                            f"{name} ({kind}) -> {actual_name} ({dependency_kind})"))
    for cycle in _cycles(graph):
        findings.append(Finding("BLOCK", "CYCLE", "Cargo.toml",
                                " -> ".join(cycle)))
    consumers: dict[str, set[str]] = defaultdict(set)
    for owner, dependencies in graph.items():
        for dependency in dependencies:
            consumers[dependency].add(owner)
    limits = policy["limits"]
    for name, users in sorted(consumers.items()):
        if kinds.get(name) == "shared":
            continue  # Shared identifiers and errors are intentionally ubiquitous.
        count = len(users)
        if count > limits["fanout_block"]:
            findings.append(Finding("BLOCK", "FANOUT", name,
                                    f"{count} direct consumers: {', '.join(sorted(users))}"))
        elif count > limits["fanout_warn"]:
            findings.append(Finding("WARN", "FANOUT", name,
                                    f"{count} direct consumers: {', '.join(sorted(users))}"))
    return findings, graph


def exception_findings(findings: list[Finding], policy: dict) -> list[Finding]:
    """Only exact, documented, non-expired exceptions; stale ones must be removed."""
    known = {"FILE_SIZE", "FANOUT"}
    result = list(findings)
    used: set[tuple[str, str]] = set()
    for item in policy.get("exceptions", []):
        rule, path = item.get("rule"), item.get("path")
        reason, issue, expires = item.get("reason"), item.get("issue"), item.get("expires")
        key = (rule, path)
        if (rule not in known or not path or "*" in path or not reason or not issue
                or not isinstance(expires, date) or expires < date.today()):
            result.append(Finding("BLOCK", "INVALID_EXCEPTION",
                                  str(path), f"invalid or expired {rule} exception"))
            continue
        matching = [f for f in result if (f.rule, f.path) == key and f.severity == "BLOCK"]
        if not matching:
            result.append(Finding("BLOCK", "STALE_EXCEPTION", str(path),
                                  f"{rule} no longer needs an exception"))
            continue
        used.add(key)
        result = [f for f in result if not (
            (f.rule, f.path) == key and f.severity == "BLOCK"
        )]
        result.append(Finding("WARN", "EXCEPTION_ACTIVE", str(path),
                              f"{rule} allowed until {expires} ({issue}): {reason}"))
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--all", action="store_true", help="check every tracked source file")
    group.add_argument("--base", help="compare PR changes to the merge base of this Git ref")
    parser.add_argument("--module-report", type=Path, help="write Rust module impact JSON")
    args = parser.parse_args(argv)
    root = args.root.resolve()
    try:
        policy = read_policy(root)
        findings = validate_policy(policy)
        paths = git_paths(root, args.base)
        findings.extend(file_findings(root, paths, policy))
        findings.extend(workflow_findings(root))
        package_findings, graph = workspace_findings(root, policy)
        findings.extend(package_findings)
        rust_graph = build_module_graph(root)
        findings.extend(Finding(item.severity, item.rule, item.path, item.detail)
                        for item in graph_findings(rust_graph, policy))
        unscanned = sorted(path for path in paths if path.endswith(".rs")
                           and "/src/" in path and path not in rust_graph.dependencies)
        for path in unscanned:
            findings.append(Finding("WARN", "RUST_FILE_UNSCANNED", path,
                                    "source file not found in declared module graph"))
        impacted_modules = rust_graph.impacted(set(paths))
        if args.module_report:
            report = {
                "scope": "explicit Rust mod/use and crate/self/super references; no macro expansion",
                "modules_scanned": rust_graph.seen_modules,
                "files_scanned": len(rust_graph.dependencies),
                "files_changed": sorted(paths),
                "impacted_rust_files": sorted(impacted_modules),
                "unscanned_rust_files": unscanned,
                "direct_dependencies": {
                    name: sorted(targets)
                    for name, targets in sorted(rust_graph.dependencies.items())
                },
                "direct_dependents": {
                    name: sorted(users)
                    for name, users in sorted(rust_graph.dependents().items())
                },
            }
            output = root / args.module_report
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n",
                              encoding="utf-8")
        findings = exception_findings(findings, policy)
    except (OSError, RuntimeError, subprocess.TimeoutExpired, tomllib.TOMLDecodeError) as exc:
        print(f"BLOCK CHECKER_ERROR: {exc}", file=sys.stderr)
        return 2

    for item in sorted(findings, key=lambda f: (f.severity != "BLOCK", f.rule, f.path)):
        print(item.render())
    count = sum(f.severity == "BLOCK" for f in findings)
    print(f"Rust modules: {rust_graph.seen_modules} modules in "
          f"{len(rust_graph.dependencies)} files; "
          f"{sum(map(len, rust_graph.dependencies.values()))} direct imports; "
          f"{len(impacted_modules)} impacted files.")
    print(f"Architecture: checked {len(paths)} changed/tracked paths; "
          f"{len(graph)} Rust packages; {count} blocking, "
          f"{sum(f.severity == 'WARN' for f in findings)} warnings.")
    return 1 if count else 0


if __name__ == "__main__":
    raise SystemExit(main())
