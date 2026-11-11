# Offline worker package contract

English · [简体中文](README.zh-CN.md)

This independent Rust library defines the signed Windows worker package format for controller assembly and native verification. The format uses a canonical UTF-8 JSON manifest with sorted file keys and one final LF, plus an Ed25519 detached signature. Verification requires an independently pinned public key; `manifest.pub` delivered beside a package cannot establish trust in that package. The signing interface returns PKCS#8 bytes to the controller caller and never serializes a private key into manifests or signatures. A protected controller key store and the complete package assembler are separate implementation work.

The manifest binds package/source versions, runtime protocol, minimum controller version, the toolchain lock file/hash, native/Python/debugger/SSH entry points, every file's SHA256/size/type and license notices. Python, Debugging Tools, OpenSSH and worker Python dependencies each require their original installation payloads and licenses. Component declarations and valid signatures do not prove vendor dependency closure or functioning tools; the assembler must check pinned official payload inventories, and actual offline VM installation/self-tests remain required.

Limits are 4 MiB per manifest, 20,000 files, 2 GiB per file and 8 GiB of payloads. Package member paths use a portable ASCII subset; installation roots can contain Unicode. Reject traversal, alternate data streams, Windows device names, trailing-dot/space aliases, case collisions and file/directory collisions. Complete-tree verification also rejects missing/extra/changed files, symlinks and Windows reparse points, including linked ancestors. Callers must keep the root private from untrusted writers throughout verification/copying; native extraction still needs protected ACLs and safe creation. These checks do not provide concurrent-write isolation by themselves.

Run the portable contract checks:

```powershell
cargo fmt --manifest-path workers/package/Cargo.toml -- --check
cargo clippy --locked --manifest-path workers/package/Cargo.toml --all-targets --target-dir target/worker-package-checks -- -D warnings
cargo test --locked --manifest-path workers/package/Cargo.toml --all-targets --target-dir target/worker-package-checks
```

Windows CI also builds the static native verifier, then explicitly runs the ignored CLI integration test with `WUKONG_WORKER_CLI` pointing to that self-built binary. It verifies an inert signed fixture and rejects a different pinned key and tampered bytes; it does not execute fixture payloads. The native command is `wukong-worker verify-package DIRECTORY PINNED_PUBLIC_KEY_HEX CONTROLLER_VERSION`. Its success covers signature, compatibility and file integrity, not installation or production isolation. See [native components](../windows/README.md) and the [environment contract](../../docs/design/cross-platform-environments.md).
