"""Guard the execution dependency boundaries, not a file/line-count target."""

import ast
from pathlib import Path
import re
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]

LINUX_WORKER = ROOT / "workers/runtime/wukong_worker"
WINDOWS_NATIVE = ROOT / "workers/windows/native"

# The resource lifecycle owns durable resources; only the host/RPC entry may
# depend on it, never the other way around.
LINUX_RESOURCE_FORBIDDEN = (
    "wukong_worker.rpc",
    "wukong_worker.host.runtime",
    "wukong_worker.cli",
)
# Platform adapters perform OS operations for the resource lifecycle and may
# use artifact primitives; anything above them would reverse the direction.
LINUX_PLATFORM_ALLOWED = ("wukong_worker.artifacts", "wukong_worker.platform")
# Real forward edges of RPC entry -> resource lifecycle -> platform adapters.
LINUX_FORWARD_DEPENDENCIES = {
    "host/runtime.py": (
        "wukong_worker.resources.jobs",
        "wukong_worker.resources.lifecycle",
        "wukong_worker.resources.operations",
        "wukong_worker.resources.sessions",
    ),
    "resources/jobs.py": (
        "wukong_worker.platform.environment",
        "wukong_worker.platform.isolation",
        "wukong_worker.platform.process_owner",
    ),
    "resources/job_ledger.py": ("wukong_worker.platform.process_control",),
    "resources/sessions.py": ("wukong_worker.platform.process_journal",),
    "platform/isolation.py": ("wukong_worker.artifacts.common",),
}

# The Windows resource layer may reference the RPC directory only through these
# exact binding-validation helpers until they move to a neutral module.
WINDOWS_RPC_EXCEPTIONS = {
    "journal.rs": {
        "crate::rpc::worker_identity::validate",
        "crate::rpc::pipe_transport::service_name",
    },
}
WINDOWS_RPC_REFERENCE = re.compile(r"crate::rpc(?:::\w+){0,2}|(?<![\w:])rpc::\w+")
WINDOWS_TEST_MODULE = re.compile(
    r"\n#\[cfg\(test\)\]\n(?:#\[path = \"[^\"]+\"\]\n)?(?:pub\(crate\)\s+)?mod\s+tests"
)
# Ledger writers name these types/modules; the recovery observer must not.
WINDOWS_LEDGER_REFERENCE = re.compile(
    r"\b(?:Journal|ResourceManager|ResourceState)\b"
    r"|(?:crate::resources|super)::(?:journal|manager|state)\b"
    r"|(?<![\w:])(?:journal|manager|state)::"
)


def _matches(name, module):
    return name == module or name.startswith(module + ".")


def _internal_imports(source):
    """`wukong_worker.*` imports of one Python file, ignoring TYPE_CHECKING annotations."""

    class Imports(ast.NodeVisitor):
        def __init__(self):
            self.names = []

        def visit_If(self, node):
            if isinstance(node.test, ast.Name) and node.test.id == "TYPE_CHECKING":
                return
            self.generic_visit(node)

        def visit_Import(self, node):
            self.names.extend(alias.name for alias in node.names)

        def visit_ImportFrom(self, node):
            self.names.append(node.module or "")

    imports = Imports()
    imports.visit(ast.parse(source))
    return [name for name in imports.names if _matches(name, "wukong_worker")]


def _forbidden_imports(names, forbidden):
    return [name for name in names if any(_matches(name, item) for item in forbidden)]


def _outside_allowlist(names, allowed):
    return [name for name in names if not any(_matches(name, item) for item in allowed)]


def _production(source):
    """Rust source before its inline test module; fixture tests are not dependencies."""
    match = WINDOWS_TEST_MODULE.search(source)
    return source[: match.start()] if match else source


class ExecutionBoundaries(unittest.TestCase):
    def test_python_worker_runtime_dependencies_follow_ownership_direction(self):
        allowed = {
            "core": set(),
            "artifacts": {"core"},
            "platform": {"artifacts"},
            "execution": {"artifacts", "platform", "core"},
            "analysis": {"artifacts", "execution", "platform", "core"},
            "debugger": {"analysis", "artifacts", "core", "execution", "platform"},
            "network": {"artifacts", "execution", "core"},
            "resources": {"analysis", "artifacts", "core", "debugger", "platform"},
            "operations": {
                "artifacts",
                "core",
                "debugger",
                "execution",
                "network",
                "platform",
                "resources",
            },
            "rpc": {"artifacts", "core", "platform", "telemetry"},
            "telemetry": set(),
        }

        root = ROOT / "workers/runtime/wukong_worker"
        for path in root.rglob("*.py"):
            owner = path.relative_to(root).parts[0]
            if owner not in allowed:
                continue
            for name in _internal_imports(path.read_text(encoding="utf-8")):
                if name.startswith("wukong_worker."):
                    dependency = name.split(".")[1]
                    self.assertIn(dependency, allowed[owner] | {owner}, f"{path}: {name}")

    def test_windows_guest_does_not_use_wfp_as_a_host_isolation_gate(self):
        native = ROOT / "workers/windows/native"
        for path in native.rglob("*.rs"):
            self.assertNotRegex(
                path.read_text(encoding="utf-8"),
                r"WindowsFilteringPlatform|FwpmNetEvent|FwpmEngine",
                str(path),
            )
        self.assertNotIn(
            "WindowsFilteringPlatform",
            (ROOT / "workers/windows/Cargo.toml").read_text(encoding="utf-8"),
        )

    def test_job_status_writes_belong_to_job_ledger(self):
        root = ROOT / "workers/runtime/wukong_worker"
        for path in root.rglob("*.py"):
            if path.name == "job_ledger.py":
                continue
            tree = ast.parse(path.read_text(encoding="utf-8"))
            for node in ast.walk(tree):
                if not isinstance(node, ast.Call) or not node.args:
                    continue
                name = node.func.id if isinstance(node.func, ast.Name) else ""
                if name == "atomic_json":
                    self.assertNotIn("status.json", ast.unparse(node.args[0]), str(path))
        archive = (root / "workbench/archive.py").read_text(encoding="utf-8")
        self.assertIn("JobLedger(path.parent).import_snapshot()", archive)

    def test_handlers_receive_concrete_services_not_the_tool_entry_point(self):
        root = ROOT / "src/application/tools"
        context = (root / "context.rs").read_text(encoding="utf-8")
        self.assertNotIn("Deref", context)
        self.assertNotRegex(context.split("impl ToolContext", 1)[0], r"\bToolService\b")
        for path in [root / "handlers.rs", *(root / "handlers").glob("*.rs")]:
            source = path.read_text(encoding="utf-8")
            self.assertNotRegex(source, r"\b(?:ToolService|ToolContext)\b", str(path))
        self.assertNotIn("use super::*", (root / "handlers.rs").read_text(encoding="utf-8"))

    def test_native_core_and_resources_do_not_route_rpc(self):
        native = ROOT / "workers/windows/native"
        for path in (native / "core").glob("*.rs"):
            self.assertNotRegex(
                path.read_text(encoding="utf-8"),
                r"crate::(?:rpc|host|artifacts|isolation|resources|executors)::",
                str(path),
            )
        for directory in ["resources", "artifacts"]:
            for path in (native / directory).glob("*.rs"):
                if path.stem.endswith("_tests"):
                    continue
                production = _production(path.read_text(encoding="utf-8"))
                self.assertNotIn("rpc::catalog", production, str(path))

    def test_linux_resources_do_not_import_the_rpc_request_entry(self):
        for path in (LINUX_WORKER / "resources").rglob("*.py"):
            violations = _forbidden_imports(
                _internal_imports(path.read_text(encoding="utf-8")), LINUX_RESOURCE_FORBIDDEN
            )
            self.assertEqual(
                violations,
                [],
                f"{path}: resource layer must not import the RPC/request entry; "
                f"forbidden dependencies {violations}",
            )

    def test_linux_platform_adapters_keep_their_dependency_direction(self):
        for path in (LINUX_WORKER / "platform").rglob("*.py"):
            violations = _outside_allowlist(
                _internal_imports(path.read_text(encoding="utf-8")), LINUX_PLATFORM_ALLOWED
            )
            self.assertEqual(
                violations,
                [],
                f"{path}: platform adapters may import only artifact primitives and their "
                f"own package; forbidden dependencies {violations} (direction is "
                "RPC entry -> resource lifecycle -> platform adapters)",
            )

    def test_linux_worker_applies_the_rpc_to_resource_to_platform_direction(self):
        for name, dependencies in LINUX_FORWARD_DEPENDENCIES.items():
            imports = _internal_imports((LINUX_WORKER / name).read_text(encoding="utf-8"))
            for dependency in dependencies:
                self.assertIn(
                    dependency,
                    imports,
                    f"{name}: missing forward dependency {dependency}; the implemented "
                    "direction is RPC entry -> resource lifecycle -> platform adapters",
                )

    def test_windows_resources_do_not_reference_the_rpc_entry(self):
        root = WINDOWS_NATIVE / "resources"
        for path in root.rglob("*.rs"):
            if path.stem.endswith("_tests"):
                continue
            allowed = WINDOWS_RPC_EXCEPTIONS.get(path.name, set())
            violations = [
                reference
                for reference in WINDOWS_RPC_REFERENCE.findall(
                    _production(path.read_text(encoding="utf-8"))
                )
                if reference not in allowed
            ]
            self.assertEqual(
                violations,
                [],
                f"{path}: resource layer references forbidden RPC dependency {violations}; "
                "only the documented binding-validation helpers may be referenced",
            )

    def test_windows_recovery_observer_cannot_write_the_resource_ledger(self):
        root = WINDOWS_NATIVE / "resources"
        observer = _production((root / "recovery_observer.rs").read_text(encoding="utf-8"))
        self.assertRegex(
            observer,
            re.compile(r"fn observe\(.*?\)\s*->\s*Result<ProtectedReceipt>", re.DOTALL),
        )
        self.assertIn("ProtectedReceipt::new(", observer)
        writer_symbols = WINDOWS_LEDGER_REFERENCE.findall(observer)
        self.assertEqual(
            writer_symbols,
            [],
            "recovery_observer.rs: external cleanup observer must only return the "
            f"protected receipt; found ledger writer references {writer_symbols}",
        )
        receipt = _production((root / "resource_recovery.rs").read_text(encoding="utf-8"))
        self.assertIn("pub(crate) struct ProtectedReceipt(Vec<u8>);", receipt)
        self.assertIn("fn verify_cleanup(&self, receipt: &ProtectedReceipt)", receipt)
        self.assertEqual(
            WINDOWS_LEDGER_REFERENCE.findall(receipt),
            [],
            "resource_recovery.rs: receipt verification must not write the ledger",
        )
        callers = {
            path.name
            for path in root.glob("*.rs")
            if not path.stem.endswith("_tests")
            and "recovery_observer::observe(" in _production(path.read_text(encoding="utf-8"))
        }
        self.assertEqual(
            callers,
            {"state.rs"},
            "recovery_observer::observe must be called only by ResourceState; "
            f"found callers {sorted(callers)}",
        )

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
            "application/tools/lab.rs",
            "application/windows_lab/provision_service.rs",
        ]:
            source = (ROOT / "src" / name).read_text(encoding="utf-8")
            self.assertNotRegex(source, r"\bbootstrap\s*::", name)

    def test_worker_relay_adapters_keep_platform_and_rpc_responsibilities_separate(self):
        root = ROOT / "src/adapters/worker"
        launch = (root / "launch.rs").read_text(encoding="utf-8")
        channel = (root / "stdio.rs").read_text(encoding="utf-8")
        self.assertNotRegex(launch, r"\b(?:jsonrpsee_core|WorkerIdentity|EnvironmentLease)\b")
        self.assertNotRegex(channel, r"\b(?:TransportConfig|WorkerIdentity|EnvironmentLease)\b")

    def test_native_installation_does_not_depend_on_setup_ui(self):
        root = ROOT / "workers/windows/native/host"
        for name in [
            "installer.rs",
            "install_service.rs",
            "install_ssh.rs",
            "installation_receipt.rs",
        ]:
            source = (root / name).read_text(encoding="utf-8")
            self.assertNotRegex(source, r"\b(?:setup_ui|MessageBoxW|GetSaveFileNameW)\b", name)

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

    def test_interfaces_do_not_access_sql_query_modules(self):
        for path in (ROOT / "src/interfaces").rglob("*.rs"):
            self.assertNotRegex(
                path.read_text(encoding="utf-8"),
                r"\b(?:storage|store)\s*::\s*(?:queries\b|\{[^{}]*\bqueries\b)",
                str(path),
            )

    def test_status_and_bundle_checks_are_independent_of_runtime_workflows(self):
        for name, forbidden in [
            (
                "application/maintenance/updates/health.rs",
                r"\b(?:tauri|interfaces)\s*::",
            ),
            (
                "application/windows_worker_setup/inventory.rs",
                r"\b(?:probe|EnvironmentRegistry|RuntimeClient|prepare_import|mark_available)\b",
            ),
            (
                "interfaces/report/bundle.rs",
                r"\b(?:Store\b|(?:domain|storage|application|queries)\s*::)",
            ),
        ]:
            source = (ROOT / "src" / name).read_text(encoding="utf-8")
            self.assertNotRegex(source, forbidden, name)

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

    def test_boundary_matchers_reject_synthetic_violations(self):
        """The guards fail on forbidden dependencies; no temporary source edit needed."""
        self.assertEqual(
            _forbidden_imports(
                [
                    "wukong_worker.resources.inventory",
                    "wukong_worker.platform.process_control",
                    "wukong_worker.rpc.server",
                    "wukong_worker.host.runtime",
                    "wukong_worker.cli",
                ],
                LINUX_RESOURCE_FORBIDDEN,
            ),
            ["wukong_worker.rpc.server", "wukong_worker.host.runtime", "wukong_worker.cli"],
        )
        self.assertEqual(
            _outside_allowlist(
                [
                    "wukong_worker.artifacts.common",
                    "wukong_worker.platform.process_control",
                    "wukong_worker.resources.jobs",
                    "wukong_worker",
                ],
                LINUX_PLATFORM_ALLOWED,
            ),
            ["wukong_worker.resources.jobs", "wukong_worker"],
        )

        def rpc_references(path, source):
            allowed = WINDOWS_RPC_EXCEPTIONS.get(path, set())
            return [
                reference
                for reference in WINDOWS_RPC_REFERENCE.findall(_production(source))
                if reference not in allowed
            ]

        self.assertEqual(
            rpc_references("journal.rs", "crate::rpc::worker_identity::validate(&binding)?;"),
            [],
        )
        self.assertEqual(
            rpc_references("journal.rs", "crate::rpc::worker::Identity"),
            ["crate::rpc::worker::Identity"],
        )
        self.assertEqual(
            rpc_references("state.rs", "use crate::{rpc::worker::Identity};"),
            ["rpc::worker"],
        )
        self.assertEqual(
            rpc_references(
                "session.rs",
                "crate::rpc::catalog::owns_resource()\n#[cfg(test)]\nmod tests {\n"
                "    crate::rpc::worker::Identity\n}\n",
            ),
            ["crate::rpc::catalog::owns_resource"],
        )
        self.assertEqual(
            WINDOWS_LEDGER_REFERENCE.findall(
                "use crate::resources::resource_recovery::ProtectedReceipt;"
            ),
            [],
        )
        for forbidden in (
            "fn observe(journal: &Journal)",
            "fn observe(manager: &ResourceManager)",
            "super::state::recover()",
            "crate::resources::journal::Ticket",
        ):
            self.assertNotEqual(
                WINDOWS_LEDGER_REFERENCE.findall(forbidden),
                [],
                f"matcher missed synthetic ledger dependency {forbidden}",
            )


if __name__ == "__main__":
    unittest.main()
