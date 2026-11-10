# Pwn 证据工具与网络验证

这些接口沿用工具注册表的 analyst/verifier 权限、同任务文件归属、worker 回执和
独立验证流程。加载方法不会增加权限；root 继续负责协调。

## seccomp_analyze

先保存有来源的过滤器为同任务文件。输入是原始 `sock_filter[]`：每条指令为
`u16 code, u8 jt, u8 jf, u32 k`，不含 `sock_fprog` 的长度或指针头。

```json
{"filter":"files/filter.bpf","endianness":"little","probes":[{"arch":3221225534,"nr":1,"args":[1,4096,16],"instruction_pointer":4198400}]}
```

`arch` 是目标 ABI 的 AUDIT_ARCH 值。示例是 x86_64 的一次 write 参数探测；不能
直接用于其他 ABI。输出保留过滤器 SHA256、指令、条件分支目标、返回动作及低
16 位动作数据。探测记录实际经过的指令路径。
省略的 syscall 参数和 instruction_pointer 按零处理，不能把它们解释成未知值。

支持常用 classic-BPF 的 word load、立即数、scratch、A/X、32 位算术、条件跳转
及返回。未知指令和未初始化 scratch 不会变成 ALLOW。解析限制为 4096 条指令、
64 个具体探测；结果仍走有界摘要与原始证据保留流程。

该工具**不提取进程当前过滤器**，也不从有限探测推出完整 allowlist。必须另外
证明过滤器来源、安装时机、线程与叠加过滤器，原生重放所需 syscall。USER_NOTIF、
TRACE 等返回还依赖外部参与者，不能解释成 syscall 最终成功。
