// Dependency test targets are not run by the controller's `cargo test --all-targets`.
// Reuse the protocol-owned semantic suite in both platform CI gates.
#[path = "../workers/protocol/tests/lifecycle.rs"]
mod lifecycle;
