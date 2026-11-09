# Pwn knowledge provenance

Reviewed 2026-11-09. These sources inform conditional methods, not unconditional exploit recipes. Pin the target's actual runtime identity; upstream versions do not account for every vendor backport.

- glibc 2.32 malloc source: https://raw.githubusercontent.com/bminor/glibc/glibc-2.32/malloc/malloc.c — Safe-Linking, alignment and duplicate-free checks.
- glibc 2.34 release: https://sourceware.org/pipermail/libc-announce/2021/000032.html — malloc hooks no longer affect the main allocator.
- how2heap: https://github.com/shellphish/how2heap — version-scoped allocator demonstrations and patch references.
- pwntools Corefile: https://docs.pwntools.com/en/stable/elf/corefile.html
- pwntools cyclic: https://docs.pwntools.com/en/stable/util/cyclic.html
- pwntools ROP: https://docs.pwntools.com/en/stable/rop/rop.html
- pwntools format strings: https://docs.pwntools.com/en/stable/fmtstr.html
- pwninit: https://github.com/io12/pwninit — matching loader/libc environment concepts.
- Linux CET: https://docs.kernel.org/arch/x86/shstk.html
- Linux seccomp: https://docs.kernel.org/userspace-api/seccomp_filter.html
- OpenSSF hardening: https://best.openssf.org/Compiler-Hardening-Guides/Compiler-Options-Hardening-Guide-for-C-and-C%2B%2B

No third-party exploit source has been copied into the runtime. Evidence of static capability, observed runtime enablement, a primitive, and demonstrated effect remain distinct.
