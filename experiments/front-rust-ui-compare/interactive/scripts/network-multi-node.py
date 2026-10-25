#!/usr/bin/env python3
"""Local TCP integration test: concurrent writers, independent nodes, and split divergence.

Requires snapshot-cli and snapshot-net built by the experiment workspace.
Two separate server processes own separate files. This does not test federation,
authentication, distributed consensus, or inter-machine network reliability.
"""
from __future__ import annotations

import json
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path


def execute(*args: str, expect_ok: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(args, text=True, capture_output=True, timeout=20)
    if expect_ok != (result.returncode == 0):
        raise AssertionError(
            f"Unexpected command result {args}: exit={result.returncode}, "
            f"stdout={result.stdout!r}, stderr={result.stderr!r}"
        )
    return result


def propose(cli: str, source: Path, target: Path, times: int) -> None:
    shutil.copyfile(source, target)
    for _ in range(times):
        execute(cli, "mutate", str(target))


def verify_status(result: subprocess.CompletedProcess[str], status: str) -> None:
    if status not in result.stdout:
        raise AssertionError(
            f"Expected {status}: exit={result.returncode}, "
            f"stdout={result.stdout}, stderr={result.stderr}"
        )


def launch_server(net: str, data: Path, ready: Path, folder: Path) -> subprocess.Popen[str]:
    if ready.exists():
        raise AssertionError("stale readiness marker")
    log = (folder / f"{ready.stem}.log").open("w", encoding="utf-8")
    try:
        process = subprocess.Popen(
            [net, "serve", str(data), str(ready)],
            stdout=log, stderr=subprocess.STDOUT, text=True,
        )
    finally:
        log.close()
    for _ in range(100):
        if ready.exists() and ready.stat().st_size:
            return process
        if process.poll() is not None:
            raise AssertionError(f"Server crashed: {process.returncode}")
        time.sleep(0.05)
    process.terminate()
    raise AssertionError("Server failed to start")


def shutdown(proc: subprocess.Popen[str]) -> None:
    if proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)


def assert_file_equal(left: Path, right: Path) -> None:
    if left.read_bytes() != right.read_bytes():
        raise AssertionError(f"Snapshot differs: {left} != {right}")


def run(cli: str, net: str, output: Path) -> dict:
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="front-rust-multinode-") as tmp:
        root = Path(tmp)
        node_a = root / "node-a"
        node_b = root / "node-b"
        node_a.mkdir()
        node_b.mkdir()
        file_a = node_a / "document.frst"
        file_b = node_b / "document.frst"
        ready_a = node_a / "ready"
        ready_b = node_b / "ready"
        execute(cli, "seed", str(file_a))
        execute(cli, "seed", str(file_b))
        procs: list[subprocess.Popen[str]] = []
        try:
            server_a = launch_server(net, file_a, ready_a, node_a)
            server_b = launch_server(net, file_b, ready_b, node_b)
            procs.extend([server_a, server_b])
            addr_a = ready_a.read_text(encoding="utf-8").strip()
            addr_b = ready_b.read_text(encoding="utf-8").strip()

            # Advisory exclusive lock must reject a second authoritative server.
            second = execute(net, "serve", str(file_a), str(node_a / "ready-second"), expect_ok=False)
            if "another server already owns data" not in second.stderr:
                raise AssertionError(f"Expected exclusive-lock rejection: {second.stderr}")

            base_a = root / "client-a-base.frst"
            base_b = root / "client-b-base.frst"
            execute(net, "get", addr_a, str(base_a))
            execute(net, "get", addr_a, str(base_b))
            assert_file_equal(base_a, base_b)

            # Proposals differ. Launch clients together; server serializes commits
            # but must never accidentally accept the other stale revision.
            proposal_a = root / "proposal-a.frst"
            proposal_b = root / "proposal-b.frst"
            propose(cli, base_a, proposal_a, 1)
            propose(cli, base_b, proposal_b, 2)
            barrier = threading.Barrier(3)
            results: dict[str, subprocess.CompletedProcess[str]] = {}
            errors: list[str] = []
            def writer(key: str, base: Path, proposed: Path) -> None:
                try:
                    barrier.wait(timeout=5)
                    results[key] = execute(net, "cas", addr_a, str(base), str(proposed),
                                           expect_ok=False if False else True)
                except AssertionError as err:
                    # A conflict is expected for one client, so capture its
                    # status with a raw subprocess below instead.
                    errors.append(str(err))
                except Exception as err:
                    errors.append(repr(err))

            # Do not use the success-checking wrapper for competing writers:
            # one of them is required to exit non-zero with CONFLICT.
            def race(key: str, base: Path, proposed: Path) -> None:
                try:
                    barrier.wait(timeout=5)
                    results[key] = subprocess.run(
                        [net, "cas", addr_a, str(base), str(proposed)],
                        text=True, capture_output=True, timeout=20,
                    )
                except Exception as err:
                    errors.append(repr(err))

            t1 = threading.Thread(target=race, args=("a", base_a, proposal_a))
            t2 = threading.Thread(target=race, args=("b", base_b, proposal_b))
            t1.start(); t2.start()
            barrier.wait(timeout=5)
            t1.join(timeout=25); t2.join(timeout=25)
            if t1.is_alive() or t2.is_alive() or errors or len(results) != 2:
                raise AssertionError(f"Concurrent clients incomplete: {errors}, {list(results)}")
            applied = [key for key, r in results.items()
                       if r.returncode == 0 and "APPLIED" in r.stdout]
            rejected = [key for key, r in results.items()
                        if r.returncode != 0 and "CONFLICT" in r.stdout]
            if len(applied) != 1 or len(rejected) != 1:
                raise AssertionError(f"Expected 1 APPLIED, 1 CONFLICT; got {results}")
            winner = proposal_a if applied[0] == "a" else proposal_b
            authoritative = root / "after-race.frst"
            execute(net, "get", addr_a, str(authoritative))
            assert_file_equal(winner, authoritative)

            # Even an exact packet replay must not create a second write.
            replay_base = base_a if applied[0] == "a" else base_b
            verify_status(execute(net, "cas", addr_a, str(replay_base), str(winner)), "REPLAY")
            execute(net, "get", addr_a, str(root / "after-replay.frst"))
            assert_file_equal(authoritative, root / "after-replay.frst")

            # A malformed length cannot crash the authority or allocate 4 GiB.
            host, port = addr_a.rsplit(":", 1)
            with socket.create_connection((host, int(port)), timeout=5) as sock:
                sock.sendall(b"FNRT" + bytes([2]) + struct.pack("<III", 1, 2, 0xFFFFFFFF))
            execute(net, "get", addr_a, str(root / "after-malformed.frst"))
            assert_file_equal(authoritative, root / "after-malformed.frst")

            # Copy a permitted snapshot from one independent node to another.
            old_b = root / "node-b-old.frst"
            execute(net, "get", addr_b, str(old_b))
            verify_status(execute(net, "cas", addr_b, str(old_b), str(authoritative)), "APPLIED")
            synced_b = root / "node-b-synced.frst"
            execute(net, "get", addr_b, str(synced_b))
            assert_file_equal(authoritative, synced_b)

            # Separate nodes may diverge. Do not silently overwrite remote work.
            split_base_a = root / "node-a-split-base.frst"
            split_base_b = root / "node-b-split-base.frst"
            execute(net, "get", addr_a, str(split_base_a))
            execute(net, "get", addr_b, str(split_base_b))
            assert_file_equal(split_base_a, split_base_b)
            next_a = root / "node-a-new.frst"
            next_b = root / "node-b-new.frst"
            propose(cli, split_base_a, next_a, 1)
            propose(cli, split_base_b, next_b, 2)
            verify_status(execute(net, "cas", addr_a, str(split_base_a), str(next_a)), "APPLIED")
            verify_status(execute(net, "cas", addr_b, str(split_base_b), str(next_b)), "APPLIED")
            stale = execute(net, "cas", addr_b, str(split_base_b), str(next_a), expect_ok=False)
            verify_status(stale, "CONFLICT")
            retained_b = root / "node-b-after-conflict.frst"
            execute(net, "get", addr_b, str(retained_b))
            assert_file_equal(next_b, retained_b)

            # Hard process restart, same underlying durable state.
            shutdown(server_b)
            procs.remove(server_b)
            ready_b.unlink()
            server_b = launch_server(net, file_b, ready_b, node_b)
            procs.append(server_b)
            addr_b = ready_b.read_text(encoding="utf-8").strip()
            recovered_b = root / "node-b-after-restart.frst"
            execute(net, "get", addr_b, str(recovered_b))
            assert_file_equal(next_b, recovered_b)

            proof = {
                "status": "PASS",
                "scope": "local loopback TCP; two independent server processes, no TLS or federation",
                "racing_requests": 2,
                "applied": 1,
                "conflict": 1,
                "exact_replay": True,
                "exclusive_lock": True,
                "malformed_length_rejected": True,
                "snapshot_replication": True,
                "divergence_rejected_by_CAS": True,
                "restart_preserved_bytes": True,
                "winner": applied[0],
                "server_a_bytes": len(next_a.read_bytes()),
                "server_b_bytes": len(next_b.read_bytes()),
            }
            output.write_text(json.dumps(proof, indent=2, sort_keys=True), encoding="utf-8")
            print("PASS two-client race, two-node replication, divergence and restart")
            return proof
        finally:
            for proc in reversed(procs):
                shutdown(proc)

if __name__ == "__main__":
    if len(sys.argv) != 4:
        raise SystemExit("usage: network-multi-node.py SNAPSHOT_CLI SNAPSHOT_NET OUTPUT.json")
    run(sys.argv[1], sys.argv[2], Path(sys.argv[3]))
