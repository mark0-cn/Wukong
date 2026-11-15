"""Guard the execution dependency boundaries, not a file/line-count target."""

import ast
from pathlib import Path
import re
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]


class ExecutionBoundaries(unittest.TestCase):
    def test_worker_imports_environment_errors_and_validation_from_their_owners(self):
        indirect = {
            "safe_env",
            "WorkerError",
            "error_kind",
            "PROTOCOL",
            "bounded_int",
            "parse_address",
            "atomic_json",
            "load_json",
            "BRIDGE_PATH",
            "MAX_RPC",
            "TERMINAL",
        }
        for path in (ROOT / "workers/runtime/wukong_worker").rglob("*.py"):
            for node in ast.walk(ast.parse(path.read_text(encoding="utf-8"))):
                if (
                    isinstance(node, ast.ImportFrom)
                    and node.module == "wukong_worker.artifacts.common"
                ):
                    self.assertFalse(
                        indirect.intersection(alias.name for alias in node.names), str(path)
                    )

    def test_worker_imports_use_explicit_package_paths(self):
        for path in (ROOT / "workers/runtime/wukong_worker").rglob("*.py"):
            for node in ast.walk(ast.parse(path.read_text(encoding="utf-8"))):
                names = (
                    [alias.name for alias in node.names]
                    if isinstance(node, ast.Import)
                    else [node.module or ""]
                    if isinstance(node, ast.ImportFrom)
                    else []
                )
                for name in names:
                    self.assertNotRegex(
                        name, r"^(worker_|workbench_|isolation$|ghidra_backend$)", str(path)
                    )

    def test_application_services_do_not_assemble_through_bootstrap(self):
        for name in [
            "application/assessment/mod.rs",
            "application/assessment/runner.rs",
            "application/tools/mod.rs",
        ]:
            source = (ROOT / "src" / name).read_text(encoding="utf-8")
            self.assertNotRegex(source, r"\bbootstrap\s*::", name)

    def test_domain_depends_on_protocol_without_host_adapters(self):
        for path in (ROOT / "src/domain").rglob("*.rs"):
            self.assertNotRegex(
                path.read_text(encoding="utf-8"),
                r"\b(?:runtime|adapters|desktop|interfaces|storage|store)\s*::",
                str(path),
            )

    def test_filesystem_capabilities_do_not_depend_on_frontend_or_maintenance(self):
        for path in (ROOT / "src/adapters/files").glob("*.rs"):
            source = path.read_text(encoding="utf-8")
            self.assertNotRegex(
                source, r"\b(?:desktop|cleanup|updates|workbench|bootstrap)\s*::", str(path)
            )
        store = (ROOT / "src/storage/mod.rs").read_text(encoding="utf-8")
        self.assertNotRegex(store, r"\b(?:cleanup|desktop)\s*::")

    def test_storage_snapshots_do_not_depend_on_desktop_presentation(self):
        for path in (ROOT / "src/storage").rglob("*.rs"):
            production = path.read_text(encoding="utf-8").split("#[cfg(test)]", 1)[0]
            self.assertNotRegex(production, r"\b(?:desktop|presentation)\s*::", str(path))
        for path in (ROOT / "src/queries").glob("*.rs"):
            source = path.read_text(encoding="utf-8")
            self.assertNotRegex(source, r"\bStore\b|\b(?:store|desktop|bootstrap)\s*::", str(path))

    def test_contract_has_no_controller_or_platform_dependencies(self):
        manifest = tomllib.loads((ROOT / "workers/protocol/Cargo.toml").read_text(encoding="utf-8"))
        for name, dependency in manifest["dependencies"].items():
            self.assertFalse(isinstance(dependency, dict) and "path" in dependency, name)
            self.assertNotRegex(name, r"windows|wukong|tokio|jsonrpsee")
        for path in (ROOT / "workers/protocol/src").rglob("*.rs"):
            self.assertNotRegex(
                path.read_text(encoding="utf-8"),
                r"crate::(?:domain|runtime|tools|isolation|host)::",
            )

    def test_analysis_has_no_worker_or_process_ownership(self):
        for path in (ROOT / "workers/analysis/wukong_analysis").glob("*.py"):
            tree = ast.parse(path.read_text(encoding="utf-8"))
            imports = []
            for node in ast.walk(tree):
                if isinstance(node, ast.Import):
                    imports.extend(n.name for n in node.names)
                if isinstance(node, ast.ImportFrom):
                    imports.append(node.module or "")
            for name in imports:
                self.assertNotRegex(
                    name,
                    r"^(worker_|wukong_worker|isolation|subprocess|multiprocessing|filelock|sqlite3)",
                    str(path),
                )

    def test_native_executors_cannot_own_or_write_the_resource_ledger(self):
        for path in (ROOT / "workers/windows/native/executors").glob("*.rs"):
            # Tests intentionally inspect protected effects through fixture-only accessors.
            production = path.read_text(encoding="utf-8").split("#[cfg(test)]", 1)[0]
            self.assertNotRegex(
                production,
                r"\b(?:Journal|ResourceManager|ResourceState)\b|resources::journal",
                str(path),
            )
            self.assertNotRegex(
                production, r"host::(?:installer|deployment)|use super::\*", str(path)
            )

    def test_linux_registry_calls_concrete_operations(self):
        for name in [
            "artifacts/files",
            "artifacts/transfers",
            "resources/jobs",
            "resources/sessions",
            "execution/commands",
            "analysis/adapters",
            "debugger/crash",
            "analysis/profile",
            "network/service",
        ]:
            tree = ast.parse(
                (ROOT / f"workers/runtime/wukong_worker/{name}.py").read_text(encoding="utf-8")
            )
            self.assertFalse(
                any(
                    isinstance(n, ast.FunctionDef) and n.name == "dispatch" for n in ast.walk(tree)
                ),
                name,
            )
        registry = (ROOT / "workers/runtime/wukong_worker/rpc/catalog.py").read_text(
            encoding="utf-8"
        )
        self.assertNotIn(".dispatch", registry)

    def test_controller_gateway_preserves_resource_ids_in_raw_receipts(self):
        source = (ROOT / "src/application/tools/gateway.rs").read_text(encoding="utf-8")
        self.assertNotRegex(source, r'\["(?:job_id|session_id)"\]\s*=')
        self.assertNotIn("map_worker", source)


if __name__ == "__main__":
    unittest.main()
