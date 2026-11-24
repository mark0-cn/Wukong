<a id="动态函数追踪与离线差分"></a>

# 动态函数追踪与离线差分

[English](dynamic-trace.md) · 简体中文

`trace_run` 和 `trace_compare` 让分析者用一份声明描述函数级观察，由 worker 在受控调试器中执行，再离线比较规范化证据。这套能力与方法无关：引擎不认识加密、解析、补丁、协议或漏洞类别。

当前后端是 **Linux x86_64 ELF + GDB/MI + SysV AMD64 ABI**。Windows 和 macOS 不广告此能力。执行前必须运行 `environment_doctor`，确认 `trace.start` 已就绪。

## 工作流程

1. 通过静态分析选择少量能区分当前假设的函数或调用点，并分配稳定的 `point_id`。
2. 为每个目标调用 `launch_prepare`。保持参数、环境、工作目录、输入模式、timeout、loader 和非目标文件一致，并提供相同输入证据。
3. 在 `TraceSpec` 中声明点位和 capture，然后以 `launch_spec`、`input`、`spec` 调用 `trace_run`。
4. 在普通后台 job 运行期间继续其他工作，使用 `job_status` 和 `job_artifacts` 获取 `trace/run.json` 与 `trace/events.jsonl`。
5. 用两组 run/events 证据调用 `trace_compare`。controller 离线验证产生证据的 job，不重新连接 worker。

调试器由 worker 私有持有，不是公开调试会话，不能通过 `debug_control` 或 `debug_command` 操作。

## TraceSpec 示例

下面的示例采集 `parse_record(bytes, size, record)`，引擎不需要了解记录算法：

```json
{
  "schema_version": 1,
  "abi": "sysv_amd64",
  "points": [
    {
      "kind": "function",
      "point_id": "parse",
      "location": {
        "kind": "symbol",
        "module_sha256": "<target-sha256>",
        "symbol": "parse_record"
      },
      "phases": ["entry", "return"],
      "capture": {
        "arguments": [
          {"name": "size", "index": 1, "value_type": "u64"}
        ],
        "buffers": [
          {
            "name": "input",
            "address_argument": 0,
            "length": {"kind": "argument", "index": 1}
          },
          {
            "name": "record",
            "address_argument": 2,
            "length": {"kind": "constant", "bytes": 16}
          }
        ],
        "return_value": "i32"
      }
    }
  ],
  "limits": {
    "max_events": 1024,
    "max_point_hits": 256,
    "max_buffer_bytes": 4096,
    "max_event_bytes": 65536,
    "max_events_bytes": 8388608,
    "stop_wait_ms": 10000
  }
}
```

比较版本时沿用同一逻辑点位，即使地址发生变化。location 也可写为 `{ "kind": "module_offset", "module_sha256": "...", "offset": 4660 }`。offset 是模块相对地址，不是文件偏移或运行时虚拟地址。

Capture 规则：

- `arguments[*].index` 以及 buffer 地址/长度参数使用从 0 开始的 SysV 整数/指针参数位置，范围 0–5。
- 整数类型为 `u8/u16/u32/u64/i8/i16/i32/i64/pointer`。
- buffer 长度使用 `{ "kind": "constant", "bytes": N }` 或 `{ "kind": "argument", "index": N }`。
- buffer 在 entry 和 return 时读取。return capture 使用 entry 保存的地址和长度，而不是已经被覆盖的参数寄存器。
- 带 `return` 的函数必须先建立 entry activation；`call_site` 只产生 `call_site` 事件，不创建 activation。
- `limits` 限制事件数、命中数、采集字节、JSONL 大小和单次 stop 等待。总 timeout 只属于 LaunchSpec。

## 离线比较

```json
{
  "left": {
    "run_evidence_id": "ev_run_left",
    "events_evidence_id": "ev_events_left"
  },
  "right": {
    "run_evidence_id": "ev_run_right",
    "events_evidence_id": "ev_events_right"
  },
  "comparison": {
    "capture_ids": ["parse:input", "parse:record", "parse:return_value"],
    "ignore": [],
    "max_differences": 16
  }
}
```

两个 job 必须属于当前调用者和同一 run，且不能是同一个 job。controller 通过 `JobRecord.imported_artifacts` 唯一反查 job，核验证据哈希和参数，不调用 `job_status`、`job_artifacts` 或 worker。因此，已经同步的证据在 worker 离线后仍可比较。

结果有四种含义：

| 结果 | 含义 |
| --- | --- |
| `match` | 选定的规范化观察在声明范围内一致。 |
| `mismatch` | 完整且可比的观察不同，包括一侧缺少事件。 |
| `incomparable` | 输入、ABI、逻辑计划、worker/backend 或非目标执行条件不兼容。 |
| `inconclusive` | 证据缺失/不完整、job 未 settled、清理或证据未就绪、发生 limit/capture error、返回未闭合，或任一侧没有事件。 |

逻辑 plan signature 排除 location 和 limits，但保留 ABI、点位类型、phase、参数位置/类型和 buffer 来源。地址变化不妨碍比较，capture 语义不同则不可比。任一侧达到 limit 都是 inconclusive。

原始 ASLR 地址、PID、时间戳、breakpoint ID 和 job ID 不参与语义相等。非模块指针保留在原始证据中，但不作为整数比较。整数按声明宽度/符号比较，buffer 按实际字节比较。buffer 差异有界，只报告首个差异 offset 等摘要，不嵌入完整内容。

## 完成与恢复

以下事实彼此独立：

1. JSONL 已关闭并完成哈希。
2. 请求的观察已完成，没有未闭合 activation、limit 或 capture error。
3. job 进入终态并确认清理。
4. controller 已同步并校验证据完整性。

部分 JSONL 前缀会保留用于诊断，但不能产生 `match`。start replay 使用稳定的 worker 请求身份，不会再次执行 trace。取消和超时会关闭私有调试器，并保留部分 run/events 产物。

`trace/run.json` 分开保存 source 与 executed target 哈希，包括 patched runtime 镜像；同时记录地址绑定、LaunchSpec、输入，以及由 worker/GDB、启动参数、环境、loader 和非目标文件组成的环境签名。

## 当前限制

- 仅 Linux x86_64 ELF + SysV AMD64；没有 Windows/macOS 后端。
- 仅前六个整数/指针参数；不支持栈参数、浮点/向量、聚合返回或任意表达式。
- 仅主 target 点位；不支持共享库、watchpoint、GDB Python 或自动调用图发现。
- 支持递归 activation。检测到多个实际执行线程会保留 partial。尾调用、longjmp、异常展开和未闭合 activation 也可能造成 partial。
- stripped PIE 只有在 mapped-file hash 和 PT_LOAD 能唯一解析时才可执行；无法解析的地址会在继续执行前拒绝。
- `match` 只覆盖声明的 capture 和输入，不证明完整程序、所有路径、安全性或利用影响等价。

原始内存和 transcript 可能包含敏感目标数据。原始证据在 controller 上哈希保存，模型只看到 bounded 摘要。crash 是一种观察，不是代码执行证明；安全影响仍须经过现有独立 finding 验证流程。

<a id="验证"></a>

## 验证

协议和 fake-MI 测试覆盖 schema、规范化 plan signature、activation 配对和 JSONL 边界。真实 Linux Docker/GDB 验收另外覆盖嵌套与递归、PIE/非 PIE/stripped 地址、调用点、buffer 修改、事件限额、不可读内存、超时、取消、清理和 replay。样本通过只证明这些机制，不代表 Agent 解题率或完整程序等价。
