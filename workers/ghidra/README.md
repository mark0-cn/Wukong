# Ghidra worker

English · [简体中文](README.zh-CN.md)

Install Ghidra and its required JDK, then set `WUKONG_GHIDRA_HEADLESS` to the absolute path of `support/analyzeHeadless` (Linux/macOS) or `support/analyzeHeadless.bat` (Windows). The controller launches it directly with arguments; no shell command templates are accepted. Ghidra 12.2 requires a Java 25 JDK: it does not auto-detect other versions, and `JAVA_HOME` must point to a supported JDK or headless startup fails with an unsupported-JDK warning and a no-TTY prompt error. The Docker worker image installs Temurin 25 LTS with a pinned checksum; local installs must set `JAVA_HOME` in the environment that starts the controller.

`GhidraBackend` imports a SHA256-verified local snapshot, runs automatic analysis, and executes `WukongBridge.java`. The Java bridge uses Ghidra's function manager, reference manager, and native decompiler. Missing dependencies, failed imports, analysis/decompile timeouts, unknown addresses and malformed results return explicit errors. These operations do not execute the target program.

Queries use **module-relative offsets from the reported image base**, never runtime addresses or file offsets. Query results contain typed address references and the original Ghidra address space name. Function listing supports bounded pagination and literal substring search; xrefs are exact-address queries; decompile accepts any address contained in a discovered function. JSON code previews are capped at 65,536 characters and the complete decompilation is retained as `decompile.c` beside `response.json`.

Analysis projects are shared only when sample SHA256, Ghidra application properties/version, launcher SHA256, bridge SHA256 and analysis configuration match. Query results add their query parameters to that identity. An initial import is followed by `-process target.bin -noanalysis` for subsequent queries. Each invocation stores request, binding, response, stdout and stderr. Project access is serialized with an exclusive `worker.lock`; a leftover lock after a controller crash requires checking the external worker is stopped before removing that one lock. A missing project or manually changed cache must be removed/rebuilt by the operator; cached results are never used across identities.

Outer process timeout terminates the process tree on Windows and the created process group on Unix. An internal Ghidra analysis timeout is rejected instead of being marked complete. A successful complete analysis becomes reusable only after the bridge has returned a valid response with the expected target hash.

The Rust binary embeds the bridge and materializes it in a content-addressed scripts directory. `script_dir` optionally selects an external bridge for development, and its content hash participates in the cache identity.

Primary API references: [Headless analyzer](https://github.com/NationalSecurityAgency/ghidra/blob/master/Ghidra/Features/Base/src/main/help/help/topics/HeadlessAnalyzer/HeadlessAnalyzer.htm), [HeadlessScript](https://github.com/NationalSecurityAgency/ghidra/blob/master/Ghidra/Features/Base/src/main/java/ghidra/app/util/headless/HeadlessScript.java), [DecompInterface](https://github.com/NationalSecurityAgency/ghidra/blob/master/Ghidra/Features/Decompiler/src/main/java/ghidra/app/decompiler/DecompInterface.java).
