<a id="worker-测试"></a>

# Worker tests

English · [简体中文](README.zh-CN.md)

- `unit/`: portable unit/trusted-local fixture tests, with explicit platform skips.
- `integration/`: actual isolated worker/Linux/GDB/AFL++/Ghidra acceptance.
- `support/`: shared protocol clients, separate from production code.

Run `python workers/tests/run.py unit` from the repository; `-p 'test_workbench*.py'` selects a subset. Full Linux unit tests need a regular user: trusted fixtures use `trusted_local`, which production bridge refuses as root. The entry point configures imports; do not discover tests by scanning runtime directories or count importing live scripts as acceptance.

Images place runtime in `/opt/wukong/runtime` and tests in `/opt/wukong/tests`, preserving the production bridge entry. Start full unit tests as a regular container user:

```sh
docker exec --user 10001:10001 wukong-worker python3 /opt/wukong/tests/run.py unit
```

Inside a worker with isolation/capabilities configured, root runs isolation acceptance and tests requiring target-UID switching:

```sh
python3 /opt/wukong/tests/run.py unit -p 'test_workbench*.py'
python3 /opt/wukong/tests/run.py integration --state /state/acceptance --fuzz
python3 /opt/wukong/tests/run.py isolation /state/isolation-acceptance
python3 /opt/wukong/tests/run.py ghidra --state /state/ghidra-acceptance
```

Use directories owned exclusively by this test run. See [worker requirements](../runtime/README.md).
