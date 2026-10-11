#!/usr/bin/env python3
"""Conservative package-level Rust test selection.

A changed package brings in every workspace package depending on it, directly
or transitively. Unknown files and root build changes select the FULL workspace.
This is NOT per-function coverage and never uses an incomplete module graph to
omit Rust tests.
"""
from __future__ import annotations

import argparse
import fnmatch
import json
import re
import subprocess
import sys
import tomllib
from collections import defaultdict, deque
from dataclasses import dataclass
from pathlib import Path


SAFE_NON_RUST = {
    "README.md", ".gitignore", ".editorconfig",
    "tools/architecture_check.py", "tools/rust_module_graph.py",
}
SAFE_PREFIXES = ("docs/", "tests/architecture/", "architecture/")


@dataclass(frozen=True)
class Selection:
    changed_files: tuple[str, ...]
    selected_packages: tuple[str, ...]
    reason: str
    full: bool

    def to_dict(self) -> dict:
        return {
            "changed_files": list(self.changed_files),
            "selected_packages": list(self.selected_packages),
            "reason": self.reason,
            "full_workspace": self.full,
            "run_tests": bool(self.selected_packages),
            "scope": "conservative reverse Cargo dependency closure",
        }


def _read_toml(path: Path) -> dict:
    with path.open("rb") as stream:
        return tomllib.load(stream)


def workspace(root: Path) -> tuple[dict[str, Path], dict[str, set[str]]]:
    manifest = _read_toml(root / "Cargo.toml")
    ws = manifest.get("workspace", {})
    if not isinstance(ws.get("members", []), list):
        raise ValueError("workspace members must be a list")
    roots: set[Path] = set()
    if "package" in manifest:
        roots.add(root / "Cargo.toml")
    for pattern in ws.get("members", []):
        roots.update(root.glob(pattern.rstrip("/") + "/Cargo.toml"))
    excluded = ws.get("exclude", [])
    if excluded:
        roots = {p for p in roots if not any(
            fnmatch.fnmatch(p.parent.relative_to(root).as_posix(), x)
            for x in excluded)}
    if not roots:
        raise ValueError("empty Rust workspace: cannot safely select tests")

    packages: dict[str, Path] = {}
    manifests: dict[str, tuple[Path, dict]] = {}
    by_directory: dict[Path, str] = {}
    for path in sorted(roots):
        data = _read_toml(path)
        name = data["package"]["name"]
        if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_-]*", name):
            raise ValueError(f"unsafe Cargo package name: {name!r}")
        if name in packages:
            raise ValueError(f"duplicate Cargo package name: {name}")
        directory = path.parent.resolve()
        packages[name] = directory
        by_directory[directory] = name
        manifests[name] = path, data

    reverse: dict[str, set[str]] = defaultdict(set)
    ws_dependencies = ws.get("dependencies", {})
    for name, (path, data) in manifests.items():
        scopes = [data]
        scopes.extend(data.get("target", {}).values())
        for scope in scopes:
            if not isinstance(scope, dict):
                continue
            for section in ("dependencies", "build-dependencies", "dev-dependencies"):
                for alias, raw in scope.get(section, {}).items():
                    if not isinstance(raw, dict):
                        continue
                    spec = raw
                    relative_to = path.parent
                    if spec.get("workspace") is True:
                        spec = ws_dependencies.get(alias)
                        if not isinstance(spec, dict):
                            continue  # An inherited external crate is not a local dependency.
                        # [workspace.dependencies] paths are relative to the root Cargo.toml.
                        relative_to = root
                    if "path" not in spec:
                        continue
                    dependency_dir = (relative_to / spec["path"]).resolve()
                    dependency_name = by_directory.get(dependency_dir)
                    if dependency_name is None:
                        raise ValueError(
                            f"{name} depends on local path outside workspace: {alias}: {dependency_dir}"
                        )
                    reverse[dependency_name].add(name)
    return packages, dict(reverse)


def changed_since(root: Path, base: str) -> list[str]:
    # --no-renames makes both sides of a rename visible.
    process = subprocess.run(
        ["git", "diff", "--no-renames", "--name-only", "--diff-filter=ACDMR",
         "-z", f"{base}...HEAD"],
        cwd=root, capture_output=True, check=False, timeout=30,
    )
    if process.returncode:
        raise RuntimeError(process.stderr.decode(errors="replace").strip())
    return [p.decode("utf-8", errors="surrogateescape")
            for p in process.stdout.split(b"\0") if p]


def select(root: Path, paths: list[str], *, force_all: bool = False) -> Selection:
    packages, reverse = workspace(root)
    everything = tuple(sorted(packages))
    changed = tuple(sorted(set(paths)))
    if force_all:
        return Selection(changed, everything, "full run requested (push or nightly)", True)
    if not changed:
        return Selection(changed, everything, "empty diff: fail safe, run all", True)

    touched: set[str] = set()
    unknown: list[str] = []
    for path in changed:
        if not path or path.startswith("/") or ".." in Path(path).parts:
            unknown.append(path)
            continue
        # Longest prefix is robust when a nested Cargo member is introduced.
        owners = [
            (len(directory.parts), name)
            for name, directory in packages.items()
            if (root / path).resolve().is_relative_to(directory)
        ]
        if owners:
            touched.add(max(owners)[1])
        elif path in SAFE_NON_RUST or path.startswith(SAFE_PREFIXES):
            continue
        else:
            unknown.append(path)
    if unknown:
        return Selection(changed, everything,
                         "unknown/build-level changes: " + ", ".join(unknown[:20]), True)
    if not touched:
        return Selection(changed, (), "documentation or architecture tools only", False)

    impacted = set(touched)
    queue = deque(sorted(touched))
    while queue:
        for dependent in sorted(reverse.get(queue.popleft(), ())):
            if dependent not in impacted:
                impacted.add(dependent)
                queue.append(dependent)
    return Selection(changed, tuple(sorted(impacted)),
                     "changed packages and reverse Cargo dependency closure: " +
                     ", ".join(sorted(touched)), False)


def main() -> int:
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    cli.add_argument("--base", help="pull request base, e.g. origin/main")
    cli.add_argument("--full", action="store_true")
    cli.add_argument("--github-output", type=Path)
    cli.add_argument("--report", type=Path, default=Path(".repo-check/test-selection.json"))
    args = cli.parse_args()
    root = args.root.resolve()
    try:
        if not args.full and not args.base:
            raise ValueError("--base or --full is required")
        paths = [] if args.full else changed_since(root, args.base)
        selected = select(root, paths, force_all=args.full)
        report = root / args.report
        report.parent.mkdir(parents=True, exist_ok=True)
        report.write_text(json.dumps(selected.to_dict(), indent=2) + "\n", encoding="utf-8")
        if args.github_output:
            with args.github_output.open("a", encoding="utf-8") as stream:
                stream.write(f"run_tests={'true' if selected.selected_packages else 'false'}\n")
                stream.write("packages=" + " ".join(selected.selected_packages) + "\n")
                stream.write(f"full={'true' if selected.full else 'false'}\n")
        print(json.dumps(selected.to_dict(), indent=2))
        return 0
    except (OSError, ValueError, KeyError, TypeError, RuntimeError,
            subprocess.TimeoutExpired, tomllib.TOMLDecodeError) as error:
        print(f"ERROR cannot safely select Rust tests: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
