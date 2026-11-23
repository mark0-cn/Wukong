<a id="observability"></a>

# 可观察性

[English](observability.md) · 简体中文

[使用指南](usage.zh-CN.md) · [文档索引](../README.zh-CN.md)

Wukong 可以导出 OpenTelemetry 信号，并从桌面端查询这些信号。本文介绍配置、本地活动、Worker trace 传播、可选开发栈及运行限制。

<a id="scope-and-source-of-truth"></a>

## 范围与权威状态

持久化业务 Store、Event、Job、回执、原始证据和验证记录仍是权威依据。Telemetry 是尽力投递的运行观察数据。缺少 span、日志、指标或后端不能证明工作没有发生，成功导出 telemetry 也不能证明业务成功。

Telemetry 不替代完成门槛、证据就绪、资源清理确认、独立验证、恢复或对账。作出这些判断时，继续使用现有工作区、job/resource 记录和原始证据。桌面端“本地活动”面板是持久化 Store 的有界投影，不是 OpenTelemetry trace。

<a id="signals-and-privacy"></a>

## 信号与隐私

控制器支持 traces、metrics、可选 OpenTelemetry logs 和本地 JSONL 日志：

- **Traces** 包含 Wukong 操作，并复用 `rig`、`rig_core`、`rig_agent` 和 `rig_ecs` 的 Rig 原生 span。run、Agent、generation、turn、call、Worker request 和 job ID 可以关联 span 与精选日志。
- **Metrics** 覆盖 run、Agent generation、Worker RPC、模型操作／Token 用量、证据同步和被丢弃的 telemetry。指标使用低基数的运行标签，刻意不把 run、Agent、call 或其他业务记录 ID 作为 metric label，因此指标不能标识单个 run 或 Agent。
- **OpenTelemetry logs 需要显式启用**（默认 `logs = false`）。OTLP logs 与本地 JSONL 都只接受显式发送到精选 `wukong_telemetry` target 的事件。任意应用诊断、`RUST_LOG` 输出和任意事件字段不会转发到这些接收端。
- **本地 JSONL 默认独立启用**。它在 `state_dir/logs` 下按天写入 `controller.jsonl*` 文件，使用有界、可丢弃、非阻塞的写入器。它是运行日志，不是证据归档。

敏感内容采集默认不启用。这些设置不提供导出原始 prompt、completion、内部推理、工具参数／结果、目标字节、凭据或完整后端错误正文的开关。标准错误仍通过既有有界诊断路径提供；精选 telemetry 接收端不是通用的详细日志导出功能。

查询适配器还会过滤返回的属性和标签、限制响应大小、脱敏响应中回显的已配置 bearer token，并返回固定安全错误而非后端响应正文。run ID 和关联 ID 仍可能属于敏感元数据，应为外部后端设置适当的访问控制与保留策略。

<a id="configuration"></a>

## 配置

配置位于 `[observability]`。导出与查询相互独立：配置查询后端不会启用导出，启用某个信号后，也必须配置 endpoint 才有 OTLP 目的地。

<a id="complete-example"></a>

### 完整示例

此示例与下文的回环开发栈匹配。该栈不使用凭据，因此省略 `credential_env`。如需 Loki／OpenTelemetry 日志导出，必须设置 `logs = true`。

```toml
[observability]
service_name = "wukong"
otlp_endpoint = "http://127.0.0.1:4318"
traces = true
metrics = true
logs = true
local_jsonl = true
log_retention_days = 7
shutdown_timeout_secs = 3

[observability.query]
grafana_url = "http://127.0.0.1:3000"
timeout_secs = 5
max_range_secs = 86400

[observability.query.tempo]
url = "http://127.0.0.1:3200"
# credential_env = "WUKONG_TEMPO_TOKEN"

[observability.query.loki]
url = "http://127.0.0.1:3100"
# credential_env = "WUKONG_LOKI_TOKEN"

[observability.query.prometheus]
url = "http://127.0.0.1:9090"
# credential_env = "WUKONG_PROMETHEUS_TOKEN"
```

| 字段 | 默认值 | 含义与接受范围 |
| --- | --- | --- |
| `service_name` | `"wukong"` | OpenTelemetry 服务名；非空，最多 128 字节。 |
| `otlp_endpoint` | 未设置 | OTLP/HTTP 基础 endpoint，控制器会追加信号路径。 |
| `traces` | `true` | 配置 endpoint 后启用 trace 导出。 |
| `metrics` | `true` | 配置 endpoint 后启用 metric 导出。 |
| `logs` | `false` | 配置 endpoint 后启用精选 OTLP log 导出。 |
| `local_jsonl` | `true` | 独立于 OTLP 导出启用精选本地 JSONL。 |
| `log_retention_days` | `7` | 本地 JSONL 保留时间，1–90 天。 |
| `shutdown_timeout_secs` | `3` | 一次总的 telemetry 排空期限，1–30 秒。 |
| `query.tempo`、`query.loki`、`query.prometheus` | 未设置 | 可选只读后端记录，包含 `url` 和可选 `credential_env`。 |
| `query.grafana_url` | 未设置 | 用于构造 Explore 深链接的 Grafana 基础 URL。 |
| `query.timeout_secs` | `5` | 后端查询超时，1–60 秒。 |
| `query.max_range_secs` | `86400` | 最大查询范围，60 秒至 31 天。 |

在桌面设置中，`credential_env` 只能填写**环境变量名**，绝不能填写 bearer token。控制器读取该变量，并把变量值作为 HTTP Bearer 凭据发送。变量缺失或无效时，只让对应查询后端降级。秘密只能留在环境变量中，不得写入 TOML、endpoint URL、Grafana 链接、截图或提交的 Compose 文件。

每个导出和查询 endpoint 都必须是绝对 HTTPS URL 或回环 HTTP URL（`localhost`、`127.0.0.1` 或 `::1`）。包含 userinfo、内嵌凭据、query 参数或 fragment 的地址会被拒绝。所有非回环目的地都必须使用 HTTPS。

<a id="export-environment-variables-and-precedence"></a>

### 导出环境变量与优先级

| 变量 | 行为 |
| --- | --- |
| `OTEL_SDK_DISABLED` | 值 `true`（不区分大小写）禁用已配置的 OTLP 导出和 Worker relay。本地 JSONL 与保存的查询配置保持独立。 |
| `OTEL_SERVICE_NAME` | 覆盖导出控制器 telemetry 使用的 `observability.service_name`。 |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | 对没有信号专属 endpoint 的所有信号，覆盖 TOML 基础 endpoint。 |
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`、`OTEL_EXPORTER_OTLP_METRICS_ENDPOINT`、`OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` | 覆盖单个信号的基础 endpoint。每个值都是该信号的完整 URL。 |
| `OTEL_EXPORTER_OTLP_HEADERS` | 以逗号分隔的 `name=value` 对提供共用 OTLP header。 |
| `OTEL_EXPORTER_OTLP_TRACES_HEADERS`、`OTEL_EXPORTER_OTLP_METRICS_HEADERS`、`OTEL_EXPORTER_OTLP_LOGS_HEADERS` | 替换单个信号的共用 header，而不是与其合并。 |

每个信号的 endpoint 优先级为：

1. 对应的 `OTEL_EXPORTER_OTLP_<SIGNAL>_ENDPOINT` 信号专属变量。
2. `OTEL_EXPORTER_OTLP_ENDPOINT`。
3. `observability.otlp_endpoint`。

基础 endpoint 会追加 `/v1/traces`、`/v1/metrics` 或 `/v1/logs`，并保留已有路径前缀。例如，traces 使用 `http://127.0.0.1:4318` 时会变为 `http://127.0.0.1:4318/v1/traces`。信号专属 endpoint 已经是完整地址：应包含完整路径，例如 `https://otel.example.com/v1/traces`；不会再追加 `/v1/<signal>` 后缀。

解析逗号分隔的键值对后，header 值会进行百分号解码。授权值只能放在环境变量中，例如 `Authorization=Bearer%20TOKEN_VALUE`；不要放入配置文件。

Telemetry 导出在加载控制器配置后只激活一次。修改导出开关、endpoint、header、服务名、采样或 `OTEL_SDK_DISABLED` 后，需要重启控制器。保存后的查询设置会用于后续查询，仅修改查询配置不需要重启控制器。

<a id="retention-shutdown-and-sampling"></a>

### 保留、关闭与采样

`log_retention_days` 只适用于控制器拥有的 `controller.jsonl*` 文件。激活 telemetry 时，修改时间早于配置截止时间的文件会被清理。它不控制 Tempo、Loki、Prometheus、Grafana、证据或业务记录的保留。开发栈有自己的七天后端保留期。

控制器关闭时，会在唯一的 `shutdown_timeout_secs` 总期限内涵盖 Worker relay 和已启用 provider 的排空。卡住或不可用的后端不能无限期阻止进程退出；待发送 telemetry 可能丢失。保留与关闭使用专门的 TOML 字段，没有自定义 Wukong 环境变量覆盖项。

控制器在激活时使用标准 OpenTelemetry SDK sampler 环境。比如，`OTEL_TRACES_SAMPLER=parentbased_traceidratio` 配合 `OTEL_TRACES_SAMPLER_ARG=0.1`，会保留已采样父 trace 中的所有 span，并对新根 trace 采样约百分之十。可按需使用 `always_on` 或 `always_off`。采样只减少 telemetry，绝不对业务执行、证据或恢复采样。Worker span 遵循传播来的 W3C 采样决定。修改采样后需要重启。

<a id="desktop-observability-view"></a>

## 桌面可观察性视图

为选中的 run 打开第四个工作区视图 **Observability**：

- **本地活动**从持久化 Store 读取当前 run、Agent、有序 Event、工具状态、job 数量和待处理证据数量。即使所有外部后端都缺失或失败，该面板仍可使用。
- **Traces**、**Logs** 和 **Metrics** 分别独立查询 Tempo、Loki 和 Prometheus。后端未配置、未授权、超时、不可用、触发限流、响应过大或格式错误时，只让对应面板降级，不会把其他信号也变成失败。
- Trace 搜索与详情保留 run/trace 关联。导出过关联信息时，日志可按 trace ID 缩小范围。
- Metrics 明确为**服务范围**。选择 run 只校验桌面上下文，不会把服务指标变成单个 run 的测量值。调查 run 级问题时，使用 traces、logs 和持久化本地活动。
- 配置 `query.grafana_url` 后，显式 Grafana 操作会打开由原生端生成的 traces、logs 或所选指标的 Explore 链接。链接携带经过校验的 run/trace 意图和所选时间范围，不携带凭据。本地栈配置了预期的 `tempo`、`loki` 和 `prometheus` datasource 标识。

`partial`、`truncated` 或 stale 提示表示查询／结果限制，不是业务状态。应跟随后续游标、缩小范围或查阅原始记录，不能把外部面板为空或被截断当作不存在的证明。

<a id="worker-trace-propagation-and-relay"></a>

## Worker trace 传播与 relay

Linux Python Worker 和 Windows 原生 Worker 使用标准 W3C `traceparent` 及可选 `tracestate` 上下文。协议 4 通过 JSON-RPC 顶层 `_wukong_telemetry` 成员携带它们，位于业务 `params` 之外。响应可以使用相同的顶层成员携带 Base64 编码的 OTLP `ExportTraceServiceRequest` 和 `ExportLogsServiceRequest` protobuf 批次。升级时必须同时更新控制器和每个 Worker：协议 3 对端会被拒绝，不会静默走兼容路径。

该旁路不重新定义 JSON-RPC `id`、method、业务参数、请求／副作用哈希、稳定请求身份、回执、幂等或重放决策。Worker 在业务分发前移除请求上下文；控制器在正常结果处理前移除响应 telemetry。无效或过大的 telemetry 可以丢弃，原始业务消息仍可使用。

Worker 把已完成 span 保存在有界的进程本地队列中。它们不获得 OTLP 后端目的地或后端凭据，也不新增 telemetry 网络连接。唯一返回路径是现有 RPC 响应信封。控制器拥有有界 relay 队列，负责解码和校验 protobuf、应用明确的 resource/span/log 属性允许清单、移除不支持的自由格式内容，并使用控制器持有的 endpoint 和 header 导出。

在队列溢出、输入无效、信号禁用、关闭或后端失败时，telemetry 队列可能丢弃数据。控制器 relay 导出时不会延迟 Worker 业务响应。指标导出启用时，`wukong_telemetry_dropped_total` 按信号和有界原因报告被丢弃的 telemetry；该指标不存在也不能证明投递完整。

<a id="local-acceptance-stack"></a>

## 本地验收栈

`scripts/observability/compose.yaml` 提供可选的**仅控制器开发验收栈**。它不是生产部署，也不连接题目或工具网络。该栈使用独立 Compose bridge，禁用出站 masquerade，宿主端口只绑定回环地址，不挂载 Docker socket，也不配置凭据。这里没有使用 Docker 的 `internal` 网络模式，因为它在 Docker Desktop 上不会发布控制器可访问的宿主端口。因此，Grafana 匿名 Viewer 访问只用于本地开发。

<a id="fixed-images-and-boundaries"></a>

### 固定镜像与边界

| 组件 | 固定镜像 |
| --- | --- |
| OpenTelemetry Collector | `otel/opentelemetry-collector:0.128.0` |
| Tempo | `grafana/tempo:2.8.1` |
| Loki | `grafana/loki:3.5.1` |
| Prometheus | `prom/prometheus:v3.4.1` |
| Grafana | `grafana/grafana:12.0.1` |

Tempo、Loki 和 Prometheus 将本地 telemetry 保留七天。它们的数据和 Grafana 状态使用 `wukong-observability` Compose 项目的命名卷。该保留期独立于控制器 JSONL 保留期。

<a id="start-health-and-stop"></a>

### 启动、健康检查与关闭

从仓库根目录运行以下命令。解析和启动可能拉取固定镜像；这些命令不会启动题目或工具容器。

```sh
docker compose -f scripts/observability/compose.yaml config
docker compose -f scripts/observability/compose.yaml up -d --wait
docker compose -f scripts/observability/compose.yaml ps
docker compose -f scripts/observability/compose.yaml logs --tail=100 collector
```

`up --wait` 和 `ps` 使用 Compose 健康检查。对应的宿主侧检查为：

```sh
curl --fail http://127.0.0.1:13133/
curl --fail http://127.0.0.1:3200/ready
curl --fail http://127.0.0.1:3100/ready
curl --fail http://127.0.0.1:9090/-/ready
curl --fail http://127.0.0.1:3000/api/health
```

Collector 容器健康检查验证已加载配置；宿主侧 `13133` 端口请求检查其实时健康 endpoint。按上面的示例把控制器基础 endpoint 配置为 `http://127.0.0.1:4318`。在 <http://127.0.0.1:3000> 打开 Grafana；固定的 **Wukong Observability** dashboard 提供 Run ID 和 Trace ID 过滤、trace 搜索／详情、日志、指标速率和 Explore 链接。

只停止此 Compose 项目：

```sh
docker compose -f scripts/observability/compose.yaml down
```

`down` 保留其命名卷。除非明确要删除此栈的本地 telemetry 数据，否则不要添加 `--volumes`。

<a id="acceptance-procedure"></a>

### 验收步骤

以下是**待执行步骤，不是真实验收已经通过的声明**：

1. 运行上述 Compose 解析／启动／健康检查命令，在测试导出前检查所有失败服务。
2. 应用示例配置并重启控制器，执行一个身份明确的分析。先检查持久化本地活动，再检查 trace 搜索／详情、启用时的精选日志、服务范围指标和每个 Grafana 深链接。
3. 分别在已配置、隔离的 Linux 和 Windows Worker 环境中执行。确认采样允许时存在关联的 Worker span，并验证业务请求 ID、结果、证据哈希、重试和恢复行为均未改变。
4. 停止或错误配置一个查询后端，验证只有对应外部面板降级，本地活动和其他外部面板仍可使用。继续之前恢复该后端。
5. 在采样、队列压力或后端中断情况下检查 telemetry 丢弃行为，不把缺失的 telemetry 解释为缺失的业务工作。
6. 使用 `down` 停止该栈；除非明确请求删除，否则保留卷。

可运行静态验证：

```sh
python -m unittest tests.automation.test_observability -v
```

验收栈运行时，可执行有界的合成 OTLP／查询预检查：

```sh
python -I scripts/observability/acceptance.py
```

它会打印供 Grafana 检查的 run／trace 对，但不能替代上述控制器、Worker 或 GUI 验收步骤。

配置解析和静态测试不能替代真实 OTLP 导出、后端查询、dashboard、原生 GUI、在线模型或 Linux／Windows Worker 验收。应另行记录实际目标、配置、包身份、已执行步骤和未覆盖范围。

<a id="query-limits-and-troubleshooting"></a>

## 查询限制与故障排查

<a id="pagination-and-bounds"></a>

### 分页与边界

- 查询分页接受 1–500 的 limit。Trace/log 游标属于其原始时间范围；本地活动使用非负 Event 序号游标。只能使用返回的游标继续，并保持范围和过滤条件一致。
- 配置的最大范围为 60 秒至 31 天，默认一天。范围过大时应缩小范围，而不是反复增加超时。
- 本地活动最多返回 256 个 Agent，过大的 Event 载荷会替换为明确的截断标记，Event 时间线上限为 1 MiB。返回 `next_cursor`／`has_more` 表示还有更多本地 Event。
- Trace 详情和 log/metric 映射均有边界。日志每行最多展示 4096 个字符；指标最多保留 32 个 series，并应用请求的点数限制。后端 JSON 响应默认上限为 2 MiB。
- `truncated` 可能表示省略了分页、span、series、点、Agent、Event 载荷或长日志行。它绝不修改底层持久记录或后端数据。

<a id="common-failures"></a>

### 常见故障

| 现象 | 检查项 |
| --- | --- |
| 所有外部面板都未配置，但本地活动正常 | 分别配置 `query.tempo`、`query.loki` 和 `query.prometheus`。仅配置导出不会配置查询。 |
| Trace 或 metric 缺失 | 配置 OTLP endpoint，保持对应信号启用，确保 `OTEL_SDK_DISABLED` 不是 `true`，重启，并检查所选时间范围和 sampler。 |
| 日志缺失 | 设置 `logs = true`，配置 Loki 查询 URL 和 OTLP logs 目的地，重启，并检查 trace/run 关联和后端保留期。本地 JSONL 是独立接收端。 |
| 一个面板报告认证失败 | 把该后端 `credential_env` 所命名的环境变量设置为有效 bearer token，然后重试。不要把 token 粘贴到设置中。 |
| 远程 HTTP endpoint 被拒绝 | 使用 HTTPS。纯 HTTP 只接受 `localhost`、`127.0.0.1` 或 `::1`；移除 userinfo、query string 和 fragment。 |
| 导出修改没有生效 | 重启控制器。仅修改查询配置时，保存后会用于后续查询。 |
| 结果 partial 或 truncated | 跟随游标、缩小时间范围／分页大小，并检查原始 Store 记录或后端数据。过滤后的查询响应不是权威证据。 |
| 过载或关闭时 telemetry 消失 | 本地写入器、Worker 或控制器队列都可能按尽力投递语义丢数据。检查可用时的 `wukong_telemetry_dropped_total`、Collector／后端健康和配置的关闭期限；绝不为重新生成 telemetry 而重放结果不确定的业务副作用。 |
| Grafana 能打开但没有数据 | 验证后端健康和 datasource 标识、所选范围／run／trace 过滤条件、信号导出和保留期。生成深链接不代表后端认证成功，也不证明存在匹配 telemetry。 |

遇到业务完成、清理、证据和恢复问题时，应回到持久化工作区和原始证据，而不是从这些运行观察视图推断状态。
