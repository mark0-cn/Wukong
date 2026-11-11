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

`toolchain.lock.json` pins official Python, OpenSSH, Python wheels and the full Debugging Tools MSI/CAB inventory by size and SHA256. The OpenSSH version in this lock is an upstream **preview**. Prepare the cache on the controller:

```powershell
python -m scripts.worker.toolchain --cache target/windows-offline-cache --download
python -m scripts.worker.toolchain --cache target/windows-offline-cache
python -m scripts.worker.layout --cache target/windows-offline-cache --output target/windows-offline-layout
```

Only `--download` permits network access. Existing mismatched cache entries fail instead of being replaced. Layout requires a Windows controller and a new output directory; it has no network/download path. It reads MSI tables in read-only mode, extracts only callback-approved CAB members, verifies the complete debugger payload inventory against metadata inside the pinned Microsoft bootstrapper, and preserves all original MSI/CAB/ZIP/wheel files and licenses. It does not run MSI installation/custom actions or modify installed tools. The extraction uses Microsoft's [MSI database](https://learn.microsoft.com/en-us/windows/win32/api/msiquery/nf-msiquery-msiviewfetch) and [cabinet callback](https://learn.microsoft.com/en-us/windows/win32/setupapi/spfilenotify-fileincabinet) APIs.

The output contains a private embeddable Python runtime with pre-unpacked wheels, Python worker sources, OpenSSH and the full SDK debugger file tree. Python ignores user/registry paths and loads only its shipped library and worker directories; the VM needs no pip. Archive paths reject links, traversal, aliases and overwrites. Use controller-private cache/staging directories: these checks do not provide concurrent-writer isolation. A failed output is retained for diagnosis and must not be reused. This command produces an **unsigned controller layout**. Ordinary controller layout checks and trusted fixtures do not establish clean Windows VM dependency coverage.

To assemble and sign a new complete payload, first commit the worker changes and rebuild from that clean commit. The assembler requires matching source revision/tree hashes, locked native build metadata, actual binary hashes and static CRT imports. It includes native binaries, their dependency license texts, the Rust standard-library copyright notice and all original third-party payloads. It never downloads missing files.

```powershell
$keys = Join-Path $PWD 'target/controller-signing'
./target/windows-worker-native/x86_64-pc-windows-msvc/release/wukong-package.exe key $keys
python -m scripts.worker.assemble --cache target/windows-offline-cache --output target/windows-worker-package --keys $keys
```

The key command prints only the public key/fingerprint for independent enrollment. Private key storage stays on the controller outside the package, protected as described in the [package contract](../package/README.md); the example uses a local development directory. `wukong-package.exe` is a controller utility and is excluded from the VM payload. The signed output binds complete files and compatibility, but still requires dependency/self-tests, the native installer/service/broker, desktop delivery/enrollment and real offline VM acceptance before production installation support can be claimed.

The native builder also compiles known x64/x86 probes and matching PDBs using the controller's MSVC toolchains and static CRT. They are hash-bound to native build metadata and copied into the signed package. The guest needs no compiler. After native signature/tree verification against the independently enrolled key, the operator/installer can run the packaged Python entry:

```powershell
./target/windows-worker-package/runtime/python/python.exe -B ./target/windows-worker-package/runtime/worker/worker_selftest.py --package target/windows-worker-package --state target/windows-worker-selftest
```

The native caller must establish package trust first and protect the version directory from writers. This entry is never a worker RPC or a replacement for signature verification. It checks shipped probe hashes before execution, exercises OpenSSH key generation without network access, and runs x64/x86 dump/live inspection and three replay trials per architecture through the packaged runtime. Temporary dummy SSH keys are removed; logs/results stay outside the signed tree. A successful report explicitly covers offline dependencies and trusted fixtures, not production target isolation or service/SSH installation. Verify the signed tree again after running it.

The debugger component is the structured DbgEng helper. Its real x64/x86 trusted-fixture acceptance is separate from production broker isolation and offline VM installation acceptance. The fixture suite also exercises replay with distinct producer/verifier IDs, three fresh process/working-directory trials per architecture, exact input/target hashes and an observed `0xc0000005` exception. This establishes the trusted replay path, without claiming production metadata protection or exploitation impact. The managed service, installer and update lifecycle are still being implemented; this build entry does not establish their availability. See the [cross-platform environment contract](../../docs/design/cross-platform-environments.md) and [runtime guide](../runtime/README.md).
