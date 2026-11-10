# Worker 测试

- `unit/`：可移植的单元和可信本地夹具测试；平台依赖项显式跳过。
- `integration/`：需要真实隔离 worker、Linux、GDB、AFL++ 或 Ghidra 的验收入口。
- `support/`：共享协议客户端，与生产运行代码分开。

从仓库运行 `python workers/tests/run.py unit`，可用 `-p 'test_workbench*.py'` 选择子集。
Linux 完整单测需以普通用户运行：其中的可信本地夹具使用 `trusted_local`，生产桥接明确拒绝以 root 开启该模式。
入口统一设置测试导入路径；不要通过扫描运行目录来发现测试，也不要把导入 live 脚本当成已执行验收。

镜像分别复制运行代码到 `/opt/re-agent/runtime`、测试到 `/opt/re-agent/tests`。生产桥接入口不变。
容器完整单测从宿主机以普通用户启动：

```sh
docker exec --user 10001:10001 re-agent-worker python3 /opt/re-agent/tests/run.py unit
```

在已经配置好隔离与能力的 worker 内，以 root 运行隔离验收及需要切换目标 UID 的专项测试：

```sh
python3 /opt/re-agent/tests/run.py unit -p 'test_workbench*.py'
python3 /opt/re-agent/tests/run.py integration --state /state/acceptance --fuzz
python3 /opt/re-agent/tests/run.py isolation /state/isolation-acceptance
python3 /opt/re-agent/tests/run.py ghidra --state /state/ghidra-acceptance
```

测试状态目录应为本轮独占目录；真实后端要求见 [worker 说明](../runtime/README.md)。
