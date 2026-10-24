# FR-ROC-001: 10k objects, AoS vs SoA, drag and derived cache

Isolated research fixture. Identical Rust semantics and sample data, **not** an
implementation of the missing FrontRust repository.

- 10,000 objects, stable typed `Id {index,generation}`, independent SoA vectors.
- 512 repeated hit-test points per timed sample; AoS and SoA must match.
- 100 selected objects moved; stale generations and zero deltas rejected.
- 100 drag previews alter ephemeral state only; committing produces one revision,
  one undo history entry; undo and no-op tested.
- Derived-only little-endian geometry cache with schema, revision, item count,
  checksum and full byte verification. Truncation, corruption and stale geometry
  rejected. It is NOT a format for accepting arbitrary untrusted document input.
- Reproducible release benchmarks record all metrics in JSON on one CI runner.

The benchmarks include cloning a scene in both move scenarios and scan the
whole scene in both hit-test variants. Such microbenchmarks are **not** an
end-to-end browser / scene graph / drag performance claim. `soa_bytes` and
`aos_bytes` are capacities of the model vectors, not OS RSS. Cache validation
does a full reserialization, so cache reload latency is not representative of
a production zero-copy cache.

Required source specification:
https://app.notion.com/p/40332a84beec8151abc5c61729fd02f8
Roc research: https://app.notion.com/p/40132a84beec813ca807c1fcd084a924

Keep PR #21 draft, do not merge into Bokarust main.
