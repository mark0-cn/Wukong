# Native Windows worker components

English · [简体中文](README.zh-CN.md)

The standalone Cargo package builds worker components without linking the desktop controller, WebView or model clients. `native/dbgeng.rs` is shared with the controller repository's development helper. The component package version is independent of the application release; the delivered worker identity also binds the actual source/binary contents and runtime protocol.

From an x64 MSVC Native Tools shell at the repository root:

```powershell
python -m scripts.worker.build_native --check
python -m scripts.worker.build_native
cargo fmt --manifest-path workers/windows/Cargo.toml -- --check
```

The builder uses the committed standalone lockfile and an explicit static CRT setting. Outputs stay under `target/windows-worker-native` by default. Before writing `native-build.json`, it inspects both normal and delay-loaded PE imports and rejects dependencies on external Visual C++/Universal CRT libraries. This check applies to the bootstrap binaries themselves; the offline bundle must separately include all dependencies and original installation payloads of third-party tools. Building performs no installation, service registration or target execution.

The implementation follows [Rust CRT linkage](https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes) and Microsoft's [PE format](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format). Windows CI formats and checks the standalone crate, builds its bootstrap binary and verifies its imports; automation tests also reject dynamic CRTs, invalid library names and malformed ordinary/delay-import tables.

`wukong-worker.exe` currently exposes `verify-package DIRECTORY PINNED_PUBLIC_KEY_HEX CONTROLLER_VERSION`, using the shared [offline package contract](../package/README.md). This read-only command verifies independent trust, compatibility and exact file contents; it does not install or activate a worker. Its executable is included in the static CRT checks alongside `wukong-dbgeng.exe`.

The debugger component is the structured DbgEng helper. Its real x64/x86 trusted-fixture acceptance is separate from production broker isolation and offline VM installation acceptance. The fixture suite also exercises replay with distinct producer/verifier IDs, three fresh process/working-directory trials per architecture, exact input/target hashes and an observed `0xc0000005` exception. This establishes the trusted replay path, without claiming production metadata protection or exploitation impact. The managed service, installer and update lifecycle are still being implemented; this build entry does not establish their availability. See the [cross-platform environment contract](../../docs/design/cross-platform-environments.md) and [runtime guide](../runtime/README.md).
