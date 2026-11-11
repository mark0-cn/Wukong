<a id="pwn-method-and-mechanism-regressions"></a>

# Pwn 方法与机制回归

[English](README.md) · 简体中文

这些是项目自行编写的可信小样本，不包含第三方题目附件、WP exp 或真实 flag。
它们覆盖可复用的不变量及反例；不构成所引用比赛题的复现，也不测量 Agent 解题率。

在 Linux x86_64 中安装 C 编译器、musl-tools 及项目工具链指定的 Rust 编译器后运行：

```sh
python -m evals.pwn.evaluate --build-dir /tmp/wukong-pwn-fixtures --require-all
python -m evals.tests.test_pwn
```

`--require-all` 将任何缺失编译器、运行时或分配复用前提的跳过视为未完成。
省略时逐项记录 skips，不能声称整套通过。Windows 不执行这些 Linux 样本。
未知题目仍通过隔离后端运行；这些源码可信的开发夹具才可使用 trusted_local。

| 样本 | 实际观察与对照 |
| --- | --- |
| stream | 真实 glibc FILE 缓冲指针控制后的输出；无 flush、无 overwrite 对照 |
| exit | return/exit、quick_exit 与立即终止的真实回调差异 |
| seccomp-fd | 已打开 FD 的原生读取；安装过滤器后 openat 返回 EPERM |
| heap-vuln / heap-fixed | stale alias 修改复用对象、realloc-to-zero 元数据；修复清除 alias；另测禁用 tcache |
| lz-vuln / lz-fixed | 自定义压缩输入的有效 back-reference、非法距离、解码越界和修复拒绝 |
| gc-vuln / gc-fixed | VM slot 复用后的旧 generation；有效 handle 对照和修复拒绝 |
| utf8 | Rust from_utf8_lossy 对合法、NUL、非法字节的真实 Cow 类型与结果字节 |
| float | C scanf binary64 对有限值、负零和 NaN payload 的位模式观察 |
| musl-printf | 静态 musl 对一个合法定位参数格式的实际输出 |

heap/LZ/GC 保留漏洞与修复版本。其余是故意提供前置原语或正常机制的控制实验，
不能作为漏洞发现正例。GC 样本只涉及逻辑 handle，不模拟完整移动式 GC；FILE
样本不提供伪 vtable，exit 样本不破坏 libc 链表。范围详见 evaluator 专用
[ground truth](manifest.json)。

构建目录保存 build.json、results.json、原始输入、源码/二进制/输入哈希、编译命令、
运行时文件哈希、实际 stdout/stderr 字节、期望值、跳过与失败。每条行为检查关联
真实执行；agent_accuracy 保持 null。缓冲效果、逻辑状态改变和原生代码执行必须
分开报告。新增结果不能替代真实 Docker、GDB 或题目环境验收。

Linux CI 使用 `--require-all` 禁止悄悄跳过机制检查，并将构建记录、原始输入、
二进制及测量结果保存为 `pwn-fixture-evidence`，保留 14 天。该产物不是正式发行包。

另外可运行真实并列容器网络验收：

```sh
python tests/acceptance/check_network_verify.py --image python:3.12-slim \
  --output-dir target/pwn-network
```

它挂载当前 worker 源码，只连接本次创建的内部 Docker 网络，检查新连接中的
capture/patch、失败响应、配置变更拒绝和 transcript 哈希；保存原始记录、源码
哈希及实际镜像 ID，最后清理本次容器和网络。它验证网络回放引擎，不证明完整
workbench 镜像、安装器或远端题目运行时相同。CI 使用固定 digest 的 Python 镜像。

盲测只给二进制及 [task 文本](tasks.json)；源码、manifest、构建元数据和预期结果
留给独立复核者。复用现有证据绑定评分器：

```sh
python -m evals.analysis.score --tasks fixtures/pwn/tasks.json \
  --build-dir /tmp/wukong-pwn-fixtures --reports reports/pwn-index.json \
  --adjudications reports/pwn-reviewed.json --output reports/pwn-metrics.json
```

独立标签须覆盖已运行任务的每个 question_id，并引用对应 report.json 中的原始
evidence_id。缺失 run 保持 unresolved；行为 harness 通过不能产生正确答案标签。
