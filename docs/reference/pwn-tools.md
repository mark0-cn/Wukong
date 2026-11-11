<a id="pwn-证据工具与网络验证"></a>

# Pwn evidence tools and network verification

English · [简体中文](pwn-tools.zh-CN.md)

These interfaces retain catalog analyst/verifier permissions, same-run file ownership, worker receipts and independent verification. Loading methods grants no permissions; root continues coordinating.

## seccomp_analyze

Save a filter with provenance as a same-run file. Input is raw `sock_filter[]`: each instruction is `u16 code, u8 jt, u8 jf, u32 k`, without the `sock_fprog` length/pointer header.

```json
{"filter":"files/filter.bpf","endianness":"little","probes":[{"arch":3221225534,"nr":1,"args":[1,4096,16],"instruction_pointer":4198400}]}
```

`arch` is the ABI's AUDIT_ARCH value. This probes an x86_64 write, not other ABIs. Output retains filter SHA256, instructions, conditional targets, return actions/low 16-bit data and actual probe instruction paths. Omitted arguments/instruction_pointer are zero, not unknown.

Supports common classic-BPF word loads, immediates, scratch, A/X, 32-bit arithmetic, conditional jumps and returns. Unknown instructions/uninitialized scratch never become ALLOW. Limits: 4096 instructions, 64 concrete probes; bounded model summaries/full original evidence still apply.

It **does not extract a running process's filters** or infer a complete allowlist from finite probes. Establish provenance, installation time, thread and stacked filters separately; natively replay needed syscalls. USER_NOTIF/TRACE also depend on external participants and do not imply final syscall success.

## libc_identify

Retains single-leak `leaked`/`symbol`, adding same-process `leaks`, same-run `candidates` and optional `target`. Without candidates, only common worker-local libc paths are examined.

```json
{"target":"targets/TARGET_HASH/sample.bin","candidates":["files/libc-a.so","files/libc-b.so"],"leaks":[{"address":139706696499200,"symbol":"puts","addend":0},{"address":139706696548352,"symbol":"read","addend":0}]}
```

Addresses illustrate schema only; real values must come from original observations of one process. `addend` is an identified within-symbol offset; base is `address - symbol_offset - addend`.

Limits: 16 candidates, 16 leaks, 32 MiB/file and 64 MiB total candidates. pyelftools statically parses ELF without loading DSOs. Results retain hashes, Build ID, architecture, width, endianness and each derivation, checking optional target ABI and preserving multiple matches. IFUNC entry offsets cannot explain resolved implementation addresses. Identical content deduplicates by hash.

`matched` remains false; `candidate_match` means only that this set's constraints match. A single candidate, aligned base or no match does not establish actual mapping. Confirm with file/process identity evidence.

<a id="poc_verify-的-network-模式"></a>

## Network mode in poc_verify

An independent verifier calls `poc_verify` for the producer's finding, then imports results through `verification_record`. Network mode requires host-prepared `files/challenge/launch.json`, service mode local_service/remote and a challenge binary matching the run's target hash. Services without local binary identity are not accepted.

Production workers connect only to internal `challenge:<configured-port>`; remote mode uses the host-managed fixed relay. Replay cannot select another host/port. Trusted local tests use loopback. Admission and every trial recheck configuration, original files and target hashes. Network plans exclude input_mode/args/env/expected.

`input` is the original send-byte file; this example contains `PING`:

```json
{
  "finding_id":"FINDING_ID",
  "target":"targets/TARGET_HASH/sample.bin",
  "input":"files/protocol.bin",
  "launch_spec":"files/challenge/launch.json",
  "trials":2,
  "network":{"steps":[
    {"op":"receive","until_base64":"cmVhZHk+IA==","max_bytes":64},
    {"op":"send","offset":0,"length":4},
    {"op":"receive","until_base64":"Cg==","max_bytes":64},
    {"op":"expect","contains_base64":"b2s6bmV0d29yay1tYXJrZXI="}
  ]}
}
```

| op | Fields/semantics |
| --- | --- |
| receive | Nonempty until_base64 delimiter; max_bytes bounds this receive; retain buffered remainder/raw chunks |
| capture | Extract unique name from latest receive offset/length, encoded hex/little/big and fitting uint64 |
| send | Send input offset/length slice; optional patches reference captured names with offset, capture, addend, width, endianness |
| expect | Match nonempty contains_base64 in new response actually received after the most recent send; final step must be expect |

Patch width is 1/2/4/8; overflow fails without implicit modulo. Recapture on every connection; never reuse another process's ASLR addresses. Limits: 2–64 steps, 1 MiB input/combined I/O budgets respectively, 64 KiB per receive, 4096 raw network chunks. Timeout, EOF, cancellation, budget overflow and failed assertions are not successes. Banners already buffered before sending cannot satisfy post-send assertions. TCP does not prove application-level causality: unread unsolicited messages may arrive later, so use a distinguishable result of this interaction, not common welcome text.

Each trial opens a new TCP connection and saves `network-transcript.json`/SHA256 with configuration binding, original I/O, steps and captures. Import checks plan, trial count, target and raw-artifact identity. Responses prove only the declared response predicate; a script printing success is not evidence.

**Limits:** declarative plaintext TCP replay, not arbitrary Python PoCs or automatic TLS. New connections do not reset services. Configuration binding does not prove deployed binary/libc/image identity matches attachments; conclusions retain `deployed runtime identity unconfirmed`. This cannot by itself establish arbitrary code execution.
