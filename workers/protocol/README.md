# Worker protocol v3

English · [简体中文](README.zh-CN.md)

Verification expectations match only populated predicates. Omitted or null optional fields do not add constraints; Rust serialization omits them. If no predicates remain, verification still requires an observed crash rather than treating a normal exit as success.

This crate owns the public operation identifiers, request types, resource observations, request context, errors and capacity contract. It depends on neither the controller nor a platform worker. JSON-RPC envelopes are handled by jsonrpsee on Windows and jsonrpcserver on Linux. SSH and stdio remain transports; private native sessions use Protobuf.

`src/methods.rs` declares each operation's input/output types, effect policy and resource demand. Platform registries bind those operations to actual handlers and readiness. The model tool catalog declares required operations directly; it does not infer them from tool names.

Generate the shared JSON Schema with `cargo run --manifest-path workers/protocol/Cargo.toml --bin export`. Add `-- --check` to verify the checked-in schema. Linux validates requests and results with jsonschema. Rust and Python consume the same `fixtures/contract.json` corpus, including invalid inputs, results, errors, identities and capability declarations.

Protocol v2 workers are rejected. Linux state records pin protocol v3 and reject older installation records; use a fresh worker state directory. Keep previous directories and evidence. There is no automatic migration or cleanup.

Side-effect receipts are separate from controller task receipts. Completed requests reuse their original result. An interrupted operation marked `once` returns `outcome_unknown`; it cannot be replayed automatically. Only the registered `reconcile` operations may consult their operation-specific durable state. Independent control requests do not wait for unrelated execution receipts.
