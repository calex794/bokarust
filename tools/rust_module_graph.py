"""Conservative, bounded graph of explicitly declared Rust modules and imports.

No macro expansion, name resolution or rust-analyzer is attempted. It reports
visible file-to-file relationships, NOT a complete call or test coverage graph.
"""
from __future__ import annotations

import re
import tomllib
from collections import defaultdict, deque
from dataclasses import dataclass, field
from pathlib import Path


@dataclass(frozen=True)
class ModuleIssue:
    severity: str
    rule: str
    path: str
    detail: str


@dataclass(frozen=True)
class UseReference:
    namespace: str
    module: tuple[str, ...]
    path: tuple[str, ...]


@dataclass
class ModuleGraph:
    dependencies: dict[str, set[str]] = field(default_factory=dict)
    issues: list[ModuleIssue] = field(default_factory=list)
    seen_modules: int = 0

    def dependents(self) -> dict[str, set[str]]:
        reverse: dict[str, set[str]] = {path: set() for path in self.dependencies}
        for source, targets in self.dependencies.items():
            for target in targets:
                reverse.setdefault(target, set()).add(source)
        return reverse

    def impacted(self, changed: set[str]) -> set[str]:
        reverse = self.dependents()
        queue = deque(path for path in sorted(changed) if path in reverse)
        visited = set(queue)
        while queue:
            for dependent in sorted(reverse[queue.popleft()]):
                if dependent not in visited:
                    visited.add(dependent)
                    queue.append(dependent)
        return visited


def _mask(source: str) -> str:
    """Keep Rust punctuation and offsets, hide comments and quoted contents."""
    output = list(source)
    length = len(source)

    def blank(first: int, last: int) -> None:
        for offset in range(first, last):
            if output[offset] not in "\n\r":
                output[offset] = " "

    i = 0
    while i < length:
        if source.startswith("//", i):
            end = source.find("\n", i)
            if end < 0:
                end = length
            blank(i, end)
            i = end
            continue
        if source.startswith("/*", i):
            start = i
            depth = 1
            i += 2
            while i < length and depth:
                if source.startswith("/*", i):
                    depth += 1
                    i += 2
                elif source.startswith("*/", i):
                    depth -= 1
                    i += 2
                else:
                    i += 1
            blank(start, i)
            continue
        raw = re.match(r'(?:br|r|rb)(?P<hash>#{0,32})"', source[i:])
        if raw:
            closing = '"' + raw.group("hash")
            start = i
            match = source.find(closing, i + len(raw.group()))
            i = length if match < 0 else match + len(closing)
            blank(start, i)
            continue
        if source[i] == '"' or (source[i:i + 2] == 'b"'):
            start = i
            if source[i] == "b":
                i += 1
            i += 1
            while i < length:
                if source[i] == "\\":
                    i += 2
                elif source[i] == '"':
                    i += 1
                    break
                else:
                    i += 1
            blank(start, min(i, length))
            continue
        # Avoid masking Rust lifetimes ('a, 'static). Only clear a char literal.
        if source[i] == "'":
            char = re.match(r"'(?:\\(?:.|u\{[a-zA-Z0-9_]+\})|[^'\\\n])'", source[i:])
            if char:
                blank(i, i + len(char.group()))
                i += len(char.group())
                continue
        i += 1
    return "".join(output)


def _tokens(masked: str) -> list[tuple[str, int]]:
    return [(m.group(), m.start()) for m in re.finditer(r"r#[A-Za-z_]\w*|[A-Za-z_]\w*|::|[{};,]", masked)]


def _split_comma(source: str) -> list[str]:
    parts: list[str] = []
    depth = 0
    beginning = 0
    for i, char in enumerate(source):
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
        elif char == "," and depth == 0:
            parts.append(source[beginning:i])
            beginning = i + 1
    parts.append(source[beginning:])
    return parts


def _use_paths(statement: str) -> list[tuple[str, ...]]:
    """Expand ordinary imports, including use crate::{a::B, c::{D, self}}."""
    expr = statement.strip()
    if not expr:
        return []
    if "{" in expr:
        start = expr.index("{")
        depth = 1
        stop = start + 1
        while stop < len(expr) and depth:
            if expr[stop] == "{":
                depth += 1
            elif expr[stop] == "}":
                depth -= 1
            stop += 1
        if depth:
            return []
        prefix = expr[:start].strip().removesuffix("::").strip()
        result: list[tuple[str, ...]] = []
        for part in _split_comma(expr[start + 1:stop - 1]):
            part = part.strip()
            if part:
                result.extend(_use_paths(prefix + "::" + part if prefix else part))
        return result
    expr = re.sub(r"\s+as\s+(?:r#)?[A-Za-z_]\w*\s*$", "", expr)
    parts = tuple(part.strip().removeprefix("r#") for part in expr.split("::") if part.strip())
    if parts and parts[-1] == "self":
        parts = parts[:-1]
    return [parts] if parts else []


def _module_search_dir(file: Path, *, crate_root: bool) -> Path:
    if crate_root or file.name == "mod.rs":
        return file.parent
    return file.parent / file.stem


def _resolve_reference(reference: UseReference,
                       module_to_file: dict[tuple[str, tuple[str, ...]], str]) -> str | None:
    path = reference.path
    if not path:
        return None
    if path[0] == "crate":
        segments = path[1:]
    elif path[0] == "self":
        segments = reference.module + path[1:]
    elif path[0] == "super":
        i = 0
        while i < len(path) and path[i] == "super":
            i += 1
        segments = reference.module[:max(0, len(reference.module) - i)] + path[i:]
    else:
        return None
    for end in range(len(segments), -1, -1):
        result = module_to_file.get((reference.namespace, segments[:end]))
        if result is not None:
            return result
    return None


def _root_files(directory: Path, manifest_data: dict) -> list[Path]:
    files: list[Path] = []
    lib = manifest_data.get("lib", {})
    bin_targets = manifest_data.get("bin", [])
    if "path" in lib:
        files.append(directory / lib["path"])
    elif (directory / "src/lib.rs").is_file():
        files.append(directory / "src/lib.rs")
    if (directory / "src/main.rs").is_file():
        files.append(directory / "src/main.rs")
    for item in bin_targets:
        if "path" in item:
            files.append(directory / item["path"])
        elif "name" in item:
            default = directory / "src/bin" / f"{item['name']}.rs"
            nested = directory / "src/bin" / item["name"] / "main.rs"
            if default.is_file():
                files.append(default)
            elif nested.is_file():
                files.append(nested)
    files.extend((directory / "src/bin").glob("*.rs"))
    files.extend((directory / "src/bin").glob("*/main.rs"))
    return sorted(set(files))


def _members(root: Path) -> list[tuple[str, Path, dict]]:
    manifest = root / "Cargo.toml"
    if not manifest.is_file():
        return []
    with manifest.open("rb") as stream:
        data = tomllib.load(stream)
    workspace = data.get("workspace", {})
    manifests: set[Path] = set()
    if "package" in data:
        manifests.add(manifest)
    for pattern in workspace.get("members", []):
        manifests.update(root.glob(f"{pattern}/Cargo.toml"))
    result: list[tuple[str, Path, dict]] = []
    for path in sorted(manifests):
        with path.open("rb") as stream:
            package = tomllib.load(stream)
        result.append((package["package"]["name"], path.parent, package))
    return result


def _scan_crate(root: Path, package: str, namespace: str, entry: Path,
                graph: ModuleGraph) -> None:
    module_to_file: dict[tuple[str, tuple[str, ...]], str] = {}
    references: list[tuple[str, UseReference]] = []
    queue: deque[tuple[Path, tuple[str, ...], Path]] = deque(
        [(entry, (), _module_search_dir(entry, crate_root=True))]
    )
    seen: set[tuple[Path, tuple[str, ...]]] = set()

    def rel(path: Path) -> str:
        return path.relative_to(root).as_posix()

    while queue:
        path, prefix, initial_directory = queue.popleft()
        if (path, prefix) in seen:
            continue
        seen.add((path, prefix))
        source = path.read_text(encoding="utf-8")
        code = _mask(source)
        tokens = _tokens(code)
        source_path = rel(path)
        graph.dependencies.setdefault(source_path, set())
        module_to_file[(namespace, prefix)] = source_path
        graph.seen_modules += 1
        inline: list[tuple[tuple[str, ...], Path, int]] = []
        inline_open: dict[int, tuple[tuple[str, ...], Path]] = {}
        depth = 0
        index = 0
        while index < len(tokens):
            word, offset = tokens[index]
            current = inline[-1][0] if inline else prefix
            directory = inline[-1][1] if inline else initial_directory
            if word == "mod" and index + 2 < len(tokens):
                child, _ = tokens[index + 1]
                delim, _ = tokens[index + 2]
                if re.fullmatch(r"(?:r#)?[A-Za-z_]\w*", child) and delim in (";", "{"):
                    child_name = child.removeprefix("r#")
                    child_path = current + (child_name,)
                    if delim == "{":
                        inline_open[index + 2] = (child_path, directory / child_name)
                        module_to_file[(namespace, child_path)] = source_path
                        graph.seen_modules += 1
                    else:
                        name_rs = directory / f"{child_name}.rs"
                        mod_rs = directory / child_name / "mod.rs"
                        existing = [p for p in (name_rs, mod_rs) if p.is_file()]
                        if len(existing) != 1:
                            graph.issues.append(ModuleIssue("WARN", "RUST_MODULE_UNRESOLVED",
                                                            source_path, f"{child_name}: expected one file, found {len(existing)}"))
                        else:
                            child_file = existing[0]
                            module_to_file[(namespace, child_path)] = rel(child_file)
                            queue.append((child_file, child_path,
                                          _module_search_dir(child_file, crate_root=False)))
            if word == "use":
                end = index + 1
                while end < len(tokens) and tokens[end][0] != ";":
                    end += 1
                if end < len(tokens):
                    expr = code[offset + len("use"):tokens[end][1]]
                    for imported in _use_paths(expr):
                        references.append((source_path, UseReference(namespace, current, imported)))
            if word in {"crate", "self", "super"} and index + 2 < len(tokens):
                if tokens[index + 1][0] == "::":
                    parts = [word]
                    k = index + 2
                    while k < len(tokens) and re.fullmatch(r"(?:r#)?[A-Za-z_]\w*", tokens[k][0]):
                        parts.append(tokens[k][0].removeprefix("r#"))
                        if k + 1 >= len(tokens) or tokens[k + 1][0] != "::":
                            break
                        k += 2
                    if len(parts) > 1:
                        references.append((source_path, UseReference(namespace, current, tuple(parts))))
            if word == "{":
                depth += 1
                if index in inline_open:
                    child_prefix, child_directory = inline_open[index]
                    inline.append((child_prefix, child_directory, depth))
            elif word == "}":
                if inline and inline[-1][2] == depth:
                    inline.pop()
                depth -= 1
            index += 1
        if re.search(r"\binclude\s*!\s*\(", code):
            graph.issues.append(ModuleIssue("WARN", "RUST_MACRO_IMPORT_UNKNOWN", source_path,
                                            "include! may introduce module dependencies"))

    for source_path, reference in references:
        target = _resolve_reference(reference, module_to_file)
        if target is not None and target != source_path:
            graph.dependencies[source_path].add(target)


def build_module_graph(root: Path) -> ModuleGraph:
    graph = ModuleGraph()
    for package, package_dir, manifest in _members(root):
        for entry in _root_files(package_dir, manifest):
            if not entry.is_file():
                graph.issues.append(ModuleIssue("BLOCK", "RUST_CRATE_ROOT_MISSING",
                                                entry.relative_to(root).as_posix(), "missing target source"))
                continue
            namespace = f"{package}/{entry.relative_to(package_dir).as_posix()}"
            _scan_crate(root, package, namespace, entry, graph)
    return graph


def _cycles(graph: dict[str, set[str]]) -> list[list[str]]:
    """Tarjan-like DFS returning one example path per discovered back edge."""
    visiting: list[str] = []
    visited: set[str] = set()
    result: list[list[str]] = []
    signatures: set[frozenset[str]] = set()

    def visit(name: str) -> None:
        visited.add(name)
        visiting.append(name)
        for target in sorted(graph[name]):
            if target in visiting:
                path = visiting[visiting.index(target):] + [target]
                signature = frozenset(path)
                if signature not in signatures:
                    signatures.add(signature)
                    result.append(path)
            elif target not in visited:
                visit(target)
        visiting.pop()

    for name in sorted(graph):
        if name not in visited:
            visit(name)
    return result


def graph_findings(graph: ModuleGraph, policy: dict) -> list[ModuleIssue]:
    findings = list(graph.issues)
    limits = policy.get("limits", {})
    reverse = graph.dependents()
    shared = policy.get("shared_modules", [])
    for name, users in sorted(reverse.items()):
        if any(name == p or name.startswith(p + "/") or name.startswith(p + ".")
               for p in shared):
            continue
        count = len(users)
        if count > limits.get("module_fanout_block", 15):
            findings.append(ModuleIssue("BLOCK", "RUST_MODULE_FANOUT", name,
                                        f"{count} direct file consumers"))
        elif count > limits.get("module_fanout_warn", 8):
            findings.append(ModuleIssue("WARN", "RUST_MODULE_FANOUT", name,
                                        f"{count} direct file consumers"))
    for path in _cycles(graph.dependencies):
        findings.append(ModuleIssue("BLOCK", "RUST_MODULE_CYCLE", path[0],
                                    " -> ".join(path)))
    owners = policy.get("module_owners", {})
    allowed = policy.get("domain_dependencies", {})

    def owner(path: str) -> str | None:
        matches = [(p, name) for p, name in owners.items()
                   if path == p or path.startswith(p + "/") or path.startswith(p + ".")]
        return max(matches, key=lambda value: len(value[0]))[1] if matches else None

    for source, targets in graph.dependencies.items():
        src_owner = owner(source)
        if src_owner is None:
            continue
        for target in targets:
            dst_owner = owner(target)
            if dst_owner and dst_owner != src_owner and dst_owner not in allowed.get(src_owner, []):
                findings.append(ModuleIssue("BLOCK", "RUST_DOMAIN_BOUNDARY", source,
                                            f"{src_owner} -> {dst_owner}: {target}"))
    return findings
