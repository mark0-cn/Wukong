"""Guard the execution dependency boundaries, not a file/line-count target."""
import ast
from pathlib import Path
import re
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]


class ExecutionBoundaries(unittest.TestCase):
    def test_contract_has_no_controller_or_platform_dependencies(self):
        manifest = tomllib.loads((ROOT / "workers/protocol/Cargo.toml").read_text(encoding="utf-8"))
        for name, dependency in manifest["dependencies"].items():
            self.assertFalse(isinstance(dependency, dict) and "path" in dependency, name)
            self.assertNotRegex(name, r"windows|wukong|tokio|jsonrpsee")
        for path in (ROOT / "workers/protocol/src").rglob("*.rs"):
            self.assertNotRegex(path.read_text(encoding="utf-8"), r"crate::(?:domain|runtime|tools|isolation|host)::")

    def test_analysis_has_no_worker_or_process_ownership(self):
        for path in (ROOT / "workers/analysis/wukong_analysis").glob("*.py"):
            tree = ast.parse(path.read_text(encoding="utf-8"))
            imports = []
            for node in ast.walk(tree):
                if isinstance(node, ast.Import): imports.extend(n.name for n in node.names)
                if isinstance(node, ast.ImportFrom): imports.append(node.module or "")
            for name in imports:
                self.assertNotRegex(name, r"^(worker_|isolation|subprocess|multiprocessing|filelock|sqlite3)", str(path))

    def test_native_executors_cannot_own_or_write_the_resource_ledger(self):
        for path in (ROOT / "workers/windows/native/executors").glob("*.rs"):
            # Tests intentionally inspect protected effects through fixture-only accessors.
            production = path.read_text(encoding="utf-8").split("#[cfg(test)]", 1)[0]
            self.assertNotRegex(production, r"\b(?:Journal|ResourceManager|ResourceState)\b|resources::journal", str(path))
            self.assertNotRegex(production, r"host::(?:installer|deployment)|use super::\*", str(path))

    def test_linux_registry_calls_concrete_operations(self):
        for name in ["worker_files", "worker_transfers", "worker_jobs", "worker_sessions", "worker_commands",
                     "worker_seccomp", "worker_crash", "worker_emu", "worker_libc", "worker_profile", "worker_network"]:
            tree = ast.parse((ROOT / f"workers/runtime/{name}.py").read_text(encoding="utf-8"))
            self.assertFalse(any(isinstance(n, ast.FunctionDef) and n.name == "dispatch" for n in ast.walk(tree)), name)
        registry = (ROOT / "workers/runtime/worker_catalog.py").read_text(encoding="utf-8")
        self.assertNotIn(".dispatch", registry)

    def test_controller_gateway_preserves_resource_ids_in_raw_receipts(self):
        source = (ROOT / "src/tools/gateway.rs").read_text(encoding="utf-8")
        self.assertNotRegex(source, r'\["(?:job_id|session_id)"\]\s*=')
        self.assertNotIn("map_worker", source)


if __name__ == "__main__":
    unittest.main()
