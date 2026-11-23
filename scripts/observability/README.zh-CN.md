# 本地 OpenTelemetry 验收栈

[English](README.md) · 简体中文

本目录提供仅用于控制器的开发验收栈。它接收宿主的 OTLP HTTP 数据，把 traces 导出到 Tempo、logs 导出到 Loki、metrics 导出到 Prometheus。Grafana 提供固定 dashboard 和 Explore 链接。该栈使用独立 Compose bridge，禁用出站 masquerade，不挂载 Docker socket，不连接题目／工具网络，也不配置凭据。宿主端口只绑定回环地址，因为 Docker 的 `internal` 网络模式在 Docker Desktop 上不会发布控制器可访问的宿主端口。Grafana 匿名访问因此只用于这些回环端口上的本地开发。

## 固定镜像

| 组件 | 镜像 |
| --- | --- |
| OpenTelemetry Collector | `otel/opentelemetry-collector:0.128.0` |
| Tempo | `grafana/tempo:2.8.1` |
| Loki | `grafana/loki:3.5.1` |
| Prometheus | `prom/prometheus:v3.4.1` |
| Grafana | `grafana/grafana:12.0.1` |

Tempo、Loki 和 Prometheus 的本地数据保留七天。它们与 Grafana 的状态只使用 `wukong-observability` Compose 项目的命名卷。

## 启动、检查和停止

从仓库根目录运行这些命令。解析和启动可能拉取固定镜像；任何命令都不会启动题目或工具容器。

```sh
docker compose -f scripts/observability/compose.yaml config
docker compose -f scripts/observability/compose.yaml up -d --wait
docker compose -f scripts/observability/compose.yaml ps
docker compose -f scripts/observability/compose.yaml logs --tail=100 collector
docker compose -f scripts/observability/compose.yaml down
```

`down` 只停止此 Compose 项目并保留其命名卷。除非任务明确要求删除该栈的本地数据，否则不要添加 `--volumes`。

把控制器的标准 OTLP 基础 endpoint 配置为 `http://127.0.0.1:4318`；控制器会追加各信号路径。在 <http://127.0.0.1:3000> 打开 Grafana。固定的 **Wukong Observability** dashboard 提供 Run ID 和 Trace ID 过滤、trace 搜索／详情、日志、指标速率以及 Explore 链接。原生只读查询适配器使用：

- Tempo：`http://127.0.0.1:3200`
- Loki：`http://127.0.0.1:3100`
- Prometheus：`http://127.0.0.1:9090`

## 健康检查

`up --wait` 和 `ps` 使用 Compose 健康检查。对应的宿主侧检查为：

```sh
curl --fail http://127.0.0.1:13133/
curl --fail http://127.0.0.1:3200/ready
curl --fail http://127.0.0.1:3100/ready
curl --fail http://127.0.0.1:9090/-/ready
curl --fail http://127.0.0.1:3000/api/health
```

Collector 容器健康检查验证已加载的配置；使用宿主侧 `13133` 请求检查其实时健康 endpoint。

运行静态验证：

```sh
python -m unittest tests.automation.test_observability -v
```

`up --wait` 成功后，用固定合成 run 验证真实 OTLP 摄入以及 Tempo、Loki、Prometheus 和 Grafana 的查询／provisioning API：

```sh
python -I scripts/observability/acceptance.py
```

探针会打印可在 Grafana 中检查的 run 和 trace ID。它不能替代真实控制器模型分析、Worker relay、原生 GUI 或平台验收。配置解析和静态测试也不能替代真实 OTLP 导出、后端查询、dashboard 或原生 GUI 验收。

运行以下命令，验证控制器实际拥有的 `TelemetryRuntime`，并通过原生查询门面检查其 traces、筛选后的日志和 metrics：

```sh
cargo test --test observability_stack controller_runtime_exports_all_signals_to_real_backends -- --ignored --test-threads=1
```

在 Windows 上，如果 Cargo 需要，请使用仓库验证过的 MSVC 入口：

```powershell
python -m scripts.dev.msvc_cargo -- test --test observability_stack controller_runtime_exports_all_signals_to_real_backends -- --ignored --test-threads=1
```

浏览器工作区验收在模拟的 Tauri IPC 边界上使用真实的 ECharts 和 vis-timeline 渲染。如有必要，先安装 Playwright Chromium，再运行 `npm run test:e2e -- observability.spec.ts`。这不是原生 Tauri 验收，也不会查询正在运行的后端。

使用从相同源码构建的隔离 Docker Worker 时，把 `WUKONG_OBSERVABILITY_WORKER_CONTAINER` 设置为它的容器名，然后运行：

```sh
cargo test --test observability_stack docker_worker_relay_exports_correlated_spans_to_tempo -- --ignored --test-threads=1
```

在 Windows 上，使用 `python -m scripts.dev.msvc_cargo -- test ...` 并保留相同的测试参数。这会验证真实的 stdio 响应 relay，并要求 Tempo 中导出的 Worker `rpc.server` span 是控制器 `wukong.worker.rpc` span 的子级。Worker 始终保持 `none` 网络，也不会获得后端 endpoint 或凭据。

也可以不使用 Docker，通过受信任的本地 Python Worker 开发夹具验证相同的 relay 和父子关系断言：

```sh
cargo test --test observability_stack trusted_local_worker_relay_exports_correlated_spans_to_tempo -- --ignored --test-threads=1
```

该受信任夹具只用于当前检出的 Worker 源码；它不是未知题目的回退，也不能替代 Docker 平台验收。
