<a id="pwn-知识来源"></a>

# Pwn knowledge provenance

English · [简体中文](pwn-knowledge.zh-CN.md)

Reviewed 2026-11-10. These sources inform conditional methods, not unconditional exploit recipes. Pin the target's actual runtime identity; upstream versions do not account for every vendor backport.

- glibc 2.32 malloc source: https://raw.githubusercontent.com/bminor/glibc/glibc-2.32/malloc/malloc.c — Safe-Linking, alignment and duplicate-free checks.
- glibc 2.34 release: https://sourceware.org/pipermail/libc-announce/2021/000032.html — malloc hooks no longer affect the main allocator.
- how2heap: https://github.com/shellphish/how2heap — version-scoped allocator demonstrations and patch references.
- pwntools Corefile: https://docs.pwntools.com/en/stable/elf/corefile.html
- pwntools cyclic: https://docs.pwntools.com/en/stable/util/cyclic.html
- pwntools dynamic resolution: https://docs.pwntools.com/en/stable/rop/ret2dlresolve.html — record staging and reliability caveats.
- pwntools ROP: https://docs.pwntools.com/en/stable/rop/rop.html
- pwntools format strings: https://docs.pwntools.com/en/stable/fmtstr.html
- pwninit: https://github.com/io12/pwninit — matching loader/libc environment concepts.
- Linux CET: https://docs.kernel.org/arch/x86/shstk.html
- Linux seccomp: https://docs.kernel.org/userspace-api/seccomp_filter.html
- OpenSSF hardening: https://best.openssf.org/Compiler-Hardening-Guides/Compiler-Options-Hardening-Guide-for-C-and-C%2B%2B

No third-party exploit source has been copied into the runtime. Evidence of static capability, observed runtime enablement, a primitive, and demonstrated effect remain distinct.

<a id="从-wp-提炼的方法"></a>

## Writeup-derived additions

The following links are the researchers' or challenge authors' own accounts.
The methods extract prerequisites and discriminating experiments, not fixed
offsets/payloads. Reading a WP is not reproduction in Wukong. Original fixtures
are documented in [the Pwn regression suite](../../fixtures/pwn/README.md).

| Primary account | Reusable observation | Method |
| --- | --- | --- |
| [kylebot, angry-FSROP](https://blog.kylebot.net/2022/10/22/angry-FSROP/) | FILE/wide-data call paths and checks must be derived from the actual build; buffer effects differ from PC control | `fsop` |
| [Jro, Cursed Grimoires, STACK The Flags 2022](https://archive.jro.sg/writeups/pwn/cursed-grimoires.html) | A relative write needs a real disclosure and a consumer; exit-time callbacks offer a different trigger from normal returns | `fsop`, `exit_handlers` |
| [0xAsta, mutedShell V1/V2](https://0xasta.me/writeups/flagyard-muted-shell/) | Existing descriptors, syscall restrictions and forbidden bytes change the attainable objective | `seccomp_io`, `payload_constraints` |
| [Aker, NiteCTF 2025 / IEEE DANCER](https://akerdidik.github.io/blog/nite-ctf-2025-writeups/) | Floating-point text is an input transformation requiring exact bit round trips | `payload_constraints` |
| [r4yan, DiceCTF 2026 / Message Store](https://blog.r4yan.com/blog/dicectf-2026-message-store) | An unchecked dispatch index and UTF-8 conversion must be analyzed with the actual call context and resulting buffer identity | `payload_constraints`, `rop_analysis` |
| [Quarkslab, HITCON 2024 / setjmp](https://blog.quarkslab.com/heap-exploitation-glibc-internals-and-nifty-tricks.html) | Internal stdio allocation/consolidation can affect a heap experiment even when menu allocations are size-limited | `heap_ptmalloc` |
| [snakeCTF authors, 2025 / old school](https://snakectf.org/writeups/2025-quals/pwn/old-school) | Tunables can disable tcache on a recent glibc; realloc-to-zero must update every alias and ownership field | `heap_ptmalloc` |
| [Med, NiteCTF 2025](https://medoabd.github.io/MyBlog/posts/nitectf25_yellowking/nitectf-2025---pwn-writeup/) | musl FILE/exit/printf assumptions must be independently recovered | `musl` |
| [Fu11shoot, DiceCTF 2026 / Garden](https://fu11shoot.com/es/writeups/dice-ctf-2026/garden/) | GC roots, overlapping compaction and off-heap object headers create invariants beyond ordinary opcode bounds | `interpreter` |
| [STAR Labs, Summer Pwnables 2025 / lz1](https://starlabs.sg/blog/2025/09-summer-pwnables-lz1-solution/) | Valid compressed inputs and decoded length/distance semantics reach bugs that invalid-header fuzzing misses | `formats`, `hunter`, `fuzz` |
| [Minh Ton, Advent of Pwn 2025 / Day 5](https://minh-ton.dev/posts/advent_of_pwn_day_5/) | io_uring is conditional on kernel/ring/opcode policy, not a universal substitute for denied syscalls | `seccomp_io` |

<a id="交叉核对与限制"></a>

## Cross-checks and deliberate limits

- [glibc 2.35 FILE dispatch definitions](https://raw.githubusercontent.com/bminor/glibc/glibc-2.35/libio/libioP.h) and [wide-file operations](https://raw.githubusercontent.com/bminor/glibc/glibc-2.35/libio/wfileops.c) ground FILE layout/check questions in one identifiable upstream release.
- [glibc 2.35 exit implementation](https://raw.githubusercontent.com/bminor/glibc/glibc-2.35/stdlib/exit.c) and [x86_64 pointer guard definitions](https://raw.githubusercontent.com/bminor/glibc/glibc-2.35/sysdeps/unix/sysv/linux/x86_64/sysdep.h) ground callback flavor, ordering and ABI-specific pointer representation. They do not establish offsets in another build.
- [musl vfprintf](https://git.musl-libc.org/cgit/musl/plain/src/stdio/vfprintf.c), [atexit](https://git.musl-libc.org/cgit/musl/plain/src/exit/atexit.c) and [stream layout](https://git.musl-libc.org/cgit/musl/plain/src/internal/stdio_impl.h) are upstream cross-checks. Claims that musl universally lacks `$` formats are excluded: its formatter contains positional-argument handling. Limits and mixing rules still depend on the implementation.
- [Rust String::from_utf8_lossy](https://doc.rust-lang.org/std/string/struct.String.html#method.from_utf8_lossy) documents borrowing valid input versus replacing invalid bytes. Actual target pointer identity still needs observation.
- [Kernel classic-BPF documentation](https://docs.kernel.org/networking/filter.html) and [seccomp filter documentation](https://docs.kernel.org/userspace-api/seccomp_filter.html) ground decoding. `seccomp_analyze` inspects one artifact and optional concrete inputs; it does not extract live filters or synthesize a global allowlist.
- `libc_identify` compares supplied ELF files and same-process symbol leaks. It preserves ambiguous candidates and rejects inconsistent bases/ABIs/IFUNC assumptions. Even one remaining candidate is not mapped-runtime confirmation.
- Declarative TCP verification records the host-prepared service configuration, target artifact hash, per-trial connection and original wire bytes. It does not reset a persistent server or prove the remote binary/libc/image equals supplied files. TLS and arbitrary Python PoC scripts are outside this replay interface.

Initial and loaded methods share the existing five-skill limit and role permissions.
These additions do not enable kernel exploitation, browser engines, another
architecture's dynamic backend, or unverified impact promotion.
