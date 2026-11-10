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

## libc_identify

保留单泄漏 `leaked`/`symbol` 调用；增加同进程 `leaks`、同任务 `candidates` 和
可选 `target`。省略 candidates 时仍只检查 worker 自身的常见 libc 路径。

```json
{"target":"targets/TARGET_HASH/sample.bin","candidates":["files/libc-a.so","files/libc-b.so"],"leaks":[{"address":139706696499200,"symbol":"puts","addend":0},{"address":139706696548352,"symbol":"read","addend":0}]}
```

示例地址只是 schema 示意，实际地址必须来自同一进程的原始观察。addend 表示
已识别的符号内偏移，基址由 `address - symbol_offset - addend` 推导。

最多 16 个候选、16 个泄漏，每文件 32 MiB、候选总计 64 MiB。使用 pyelftools
静态解析 ELF，不加载 DSO。结果保存哈希、Build ID、架构、位宽、字节序及每个
泄漏的推导，检查可选目标 ABI，并保留多个匹配。不支持用 IFUNC 符号入口偏移
解释已解析的实现地址。同内容候选按哈希去重。

`matched` 保持 false；`candidate_match` 仅表示在该集合中满足约束。单一候选、
对齐基址和无匹配都不能确定真实映射，仍须通过实际文件/进程身份证据确认。

## poc_verify 的 network 模式

独立 verifier 继续对生产者 finding 调用 `poc_verify`，完成后通过
`verification_record` 导入结果。网络模式要求固定的宿主准备文件
`files/challenge/launch.json`，其中服务模式必须为 local_service 或 remote，
并有与本任务样本哈希匹配的题目 binary。当前不接受无本地 binary 身份的服务。

生产 worker 只连接内部 `challenge:<配置端口>`；remote 模式沿用宿主管理的固定
转发。模型不能在 replay 中指定另一个 host/port。可信本地测试使用回环地址。
配置、原始文件和目标哈希在准入及每次试验重新检查。网络计划与
input_mode/args/env/expected 互斥。

input 是原始发送字节文件。下面示意其内容为 `PING`：

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

步骤及数据流：

| op | 字段与语义 |
| --- | --- |
| receive | until_base64 为非空分隔符，max_bytes 限定本次接收；保留剩余缓冲与原始网络块 |
| capture | 从最近 receive 的 offset/length 提取唯一 name，encoding 为 hex/little/big；值须落在 uint64 |
| send | 发送 input 的 offset/length 切片；可选 patches 引用已捕获 name，指定 offset、capture、addend、width 与 endianness |
| expect | 在最近一次发送之后实际收到的新响应中匹配非空 contains_base64；最后一步必须是 expect |

patch 的 width 为 1/2/4/8，溢出会失败，不隐式取模。每次连接重新提取捕获值，
不能沿用上一进程的 ASLR 地址。步骤数为 2–64，input/收发总字节各不超过 1 MiB，
每个 receive 不超过 64 KiB，网络原始块不超过 4096。超时、EOF、取消、预算超限、
断言失败均不算成功。发送前已读取到缓冲区的 banner 不能满足发送后的成功断言。
TCP 不提供应用消息的因果证明；尚未读取的主动推送仍可能随后到达，因此成功谓词
应使用本次交互的可区分结果，不能仅匹配服务常见的欢迎文字。

每次试验建立新 TCP 连接，保存含配置绑定、原始收发、步骤结果和捕获值的
network-transcript.json 及 SHA256；导入验证记录时核对计划、次数、目标和原始
产物身份。网络响应只证明声明的响应谓词；脚本打印的成功字样不是证据。

**限制：**当前是明文 TCP 声明式回放，不执行任意 Python PoC、不自动处理 TLS。
新连接不代表服务已重置。配置绑定不证明远端 binary/libc/镜像与附件相同，验证
结论明确保留 deployed runtime identity unconfirmed。不能据此升级为任意代码执行。
