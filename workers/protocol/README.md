# Worker protocol v3

English · [简体中文](README.zh-CN.md)

Verification expectations match only populated predicates. Omitted or null optional fields do not add constraints; Rust serialization omits them. If no predicates remain, verification still requires an observed crash rather than treating a normal exit as success.

`job.list` and `resource.list` accept an optional exclusive `after` cursor and a `limit` of 1–64 (default 64). Job IDs are returned in ascending order; continue after the last ID until an empty page. Resource pages also expose `next_after`. Linux job inventory honors the same cursor and bounds as native Windows.

`debug.start` can select a staged `target` or a staged `launch_spec`; the controller checks every supplied launch/input artifact against the current run. A native Windows LaunchSpec owns the target and arguments, so omit per-call `target` and `args` in that mode. The default debugger mode is `launch` and the default engine is `auto`. Linux derives the original target hash from the checked LaunchSpec before resolving its execution environment.

`exec.run`, `crash.replay`, `input.minimize` and `poc.verify` accept an optional `output_limit` of 1,024–8,388,608 bytes (default 1 MiB), matching both platform executors. The limit bounds retained execution output (per trial for replay/minimization); it is distinct from the controller's model-facing output preview. Producer/verifier identities belong only to `poc.verify` requests.

This crate owns the public operation identifiers, request types, resource observations, request context, errors and capacity contract. It depends on neither the controller nor a platform worker. JSON-RPC envelopes are handled by jsonrpsee on Windows and jsonrpcserver on Linux. SSH and stdio remain transports; private native sessions use Protobuf.

`src/methods.rs` declares each operation's input/output types, effect policy and resource demand. Platform registries bind those operations to actual handlers and readiness. The model tool catalog declares required operations directly; it does not infer them from tool names.

Generate the shared JSON Schema with `cargo run --manifest-path workers/protocol/Cargo.toml --bin export`. Add `-- --check` to verify the checked-in schema. Linux validates requests and results with jsonschema. Rust and Python consume the same `fixtures/contract.json` corpus, including invalid inputs, results, errors, identities and capability declarations.

Protocol v2 workers are rejected. Linux state records pin protocol v3 and reject older installation records; use a fresh worker state directory. Keep previous directories and evidence. There is no automatic migration or cleanup.

Side-effect receipts are separate from controller task receipts. Completed requests reuse their original result. An interrupted operation marked `once` returns `outcome_unknown`; it cannot be replayed automatically. Only the registered `reconcile` operations may consult their operation-specific durable state. Independent control requests do not wait for unrelated execution receipts.
