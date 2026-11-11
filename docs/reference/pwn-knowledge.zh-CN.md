<a id="pwn-knowledge-provenance"></a>

# Pwn 知识来源

[English](pwn-knowledge.md) · 简体中文

审阅日期：2026-11-10。来源用于有前提的方法，不是无条件利用配方。固定目标实际运行时身份；上游版本不能涵盖所有发行版回补。

- glibc 2.32 malloc：https://raw.githubusercontent.com/bminor/glibc/glibc-2.32/malloc/malloc.c — Safe-Linking、对齐及重复释放检查。
- glibc 2.34 发布：https://sourceware.org/pipermail/libc-announce/2021/000032.html — malloc hooks 不再影响主分配器。
- how2heap：https://github.com/shellphish/how2heap — 按版本的分配器示例与补丁。
- pwntools Corefile：https://docs.pwntools.com/en/stable/elf/corefile.html
- pwntools cyclic：https://docs.pwntools.com/en/stable/util/cyclic.html
- pwntools 动态解析：https://docs.pwntools.com/en/stable/rop/ret2dlresolve.html — 记录暂存和可靠性限制。
- pwntools ROP：https://docs.pwntools.com/en/stable/rop/rop.html
- pwntools 格式串：https://docs.pwntools.com/en/stable/fmtstr.html
- pwninit：https://github.com/io12/pwninit — 匹配 loader/libc 的环境概念。
- Linux CET：https://docs.kernel.org/arch/x86/shstk.html
- Linux seccomp：https://docs.kernel.org/userspace-api/seccomp_filter.html
- OpenSSF 加固：https://best.openssf.org/Compiler-Hardening-Guides/Compiler-Options-Hardening-Guide-for-C-and-C%2B%2B

运行时未复制第三方利用源码。静态能力、运行时启用观察、原语和已展示效果保持分离。

<a id="writeup-derived-additions"></a>

## 从 WP 提炼的方法

下列为研究者或出题者原文。提炼前提及可区分假设的实验，不固定偏移/payload。读过 WP 不等于在 Wukong 复现。原创夹具见 [Pwn 回归套件](../../fixtures/pwn/README.zh-CN.md)。

| 原始资料 | 可复用观察 | 方法 |
| --- | --- | --- |
| [kylebot, angry-FSROP](https://blog.kylebot.net/2022/10/22/angry-FSROP/) | 从实际构建推导 FILE/wide-data 路径与检查；缓冲效果不同于 PC 控制 | `fsop` |
| [Jro, Cursed Grimoires, STACK The Flags 2022](https://archive.jro.sg/writeups/pwn/cursed-grimoires.html) | 相对写需要真实泄漏与消费路径；退出回调的触发不同于普通 return | `fsop`, `exit_handlers` |
| [0xAsta, mutedShell V1/V2](https://0xasta.me/writeups/flagyard-muted-shell/) | 既有 FD、syscall 限制和禁用字节改变可达目标 | `seccomp_io`, `payload_constraints` |
| [Aker, NiteCTF 2025 / IEEE DANCER](https://akerdidik.github.io/blog/nite-ctf-2025-writeups/) | 浮点文本转换要求精确位模式往返验证 | `payload_constraints` |
| [r4yan, DiceCTF 2026 / Message Store](https://blog.r4yan.com/blog/dicectf-2026-message-store) | 未检查分派索引和 UTF-8 转换需结合真实调用上下文及结果缓冲身份 | `payload_constraints`, `rop_analysis` |
| [Quarkslab, HITCON 2024 / setjmp](https://blog.quarkslab.com/heap-exploitation-glibc-internals-and-nifty-tricks.html) | 菜单限制分配大小时，stdio 内部分配/合并仍会影响实验 | `heap_ptmalloc` |
| [snakeCTF authors, 2025 / old school](https://snakectf.org/writeups/2025-quals/pwn/old-school) | 新 glibc 的 tunables 可关闭 tcache；realloc-to-zero 必须更新所有 alias/owner 字段 | `heap_ptmalloc` |
| [Med, NiteCTF 2025](https://medoabd.github.io/MyBlog/posts/nitectf25_yellowking/nitectf-2025---pwn-writeup/) | musl FILE/exit/printf 假设需独立还原 | `musl` |
| [Fu11shoot, DiceCTF 2026 / Garden](https://fu11shoot.com/es/writeups/dice-ctf-2026/garden/) | GC roots、重叠压缩及 off-heap 对象头带来 opcode 边界之外的不变量 | `interpreter` |
| [STAR Labs, Summer Pwnables 2025 / lz1](https://starlabs.sg/blog/2025/09-summer-pwnables-lz1-solution/) | 有效压缩输入与解码长度/距离可触达坏头部 fuzz 遗漏的漏洞 | `formats`, `hunter`, `fuzz` |
| [Minh Ton, Advent of Pwn 2025 / Day 5](https://minh-ton.dev/posts/advent_of_pwn_day_5/) | io_uring 取决于内核/ring/opcode 策略，不是被禁 syscall 的通用替代 | `seccomp_io` |

<a id="cross-checks-and-deliberate-limits"></a>

## 交叉核对与限制

- [glibc 2.35 FILE 分派](https://raw.githubusercontent.com/bminor/glibc/glibc-2.35/libio/libioP.h)及 [wide-file 操作](https://raw.githubusercontent.com/bminor/glibc/glibc-2.35/libio/wfileops.c)将布局/检查绑定到可识别版本。
- [glibc 2.35 exit](https://raw.githubusercontent.com/bminor/glibc/glibc-2.35/stdlib/exit.c)及 [x86_64 pointer guard](https://raw.githubusercontent.com/bminor/glibc/glibc-2.35/sysdeps/unix/sysv/linux/x86_64/sysdep.h)支撑回调类型、顺序及 ABI 指针表示，不确定其他构建的偏移。
- [musl vfprintf](https://git.musl-libc.org/cgit/musl/plain/src/stdio/vfprintf.c)、[atexit](https://git.musl-libc.org/cgit/musl/plain/src/exit/atexit.c)、[流布局](https://git.musl-libc.org/cgit/musl/plain/src/internal/stdio_impl.h)用于核对。不采用“musl 完全不支持 `$` 格式”的说法，其实现有定位参数处理；限制/混用规则仍取决于实现。
- [Rust String::from_utf8_lossy](https://doc.rust-lang.org/std/string/struct.String.html#method.from_utf8_lossy)说明有效输入借用、非法字节替换；目标真实指针身份仍需观察。
- [内核 classic-BPF](https://docs.kernel.org/networking/filter.html)与 [seccomp](https://docs.kernel.org/userspace-api/seccomp_filter.html)用于解码依据。`seccomp_analyze` 检查一个产物及可选具体输入，不提取在线过滤器或合成全局 allowlist。
- `libc_identify` 比较提供的 ELF 和同进程符号泄漏，保留歧义，拒绝不一致基址/ABI/IFUNC 假设。唯一剩余候选也不代表真实映射确认。
- 声明式 TCP 验证记录宿主准备的服务配置、目标哈希、逐次连接和原始字节。不重置持久服务，也不证明远端 binary/libc/image 等于附件。不支持 TLS 或任意 Python PoC。

初始/动态方法共享最多五个 Skill 及既有角色权限。新增方法不启用内核利用、浏览器引擎、其他架构动态后端，也不允许未经验证提升影响。
