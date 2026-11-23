"""Validate the reproducible, loopback-only observability development stack.

The automation environment intentionally has no YAML dependency. A strict parser
for the small YAML subset used by these files checks indentation, duplicate keys,
and scalar/list/map structure without adding a runtime or CI dependency.
"""

import ast
import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
STACK = ROOT / "scripts/observability"
PAIR = re.compile(r"^([A-Za-z0-9_./-]+):(?:\s+(.*))?$")
INTEGER = re.compile(r"^-?(?:0|[1-9][0-9]*)$")


class StrictYaml:
    def __init__(self, source, path):
        self.path = path
        self.lines = []
        for number, raw in enumerate(source.splitlines(), 1):
            if "\t" in raw:
                self.fail(number, "tabs are not allowed")
            stripped = raw.strip()
            if not stripped or stripped.startswith("#"):
                continue
            indent = len(raw) - len(raw.lstrip(" "))
            if indent % 2:
                self.fail(number, "indentation must use two-space steps")
            self.lines.append((indent, raw[indent:], number))

    def fail(self, number, message):
        raise AssertionError(f"{self.path}:{number}: {message}")

    def scalar(self, text, number):
        if text.startswith('"'):
            try:
                value = json.loads(text)
            except json.JSONDecodeError as error:
                self.fail(number, f"invalid double-quoted scalar: {error}")
            if not isinstance(value, str):
                self.fail(number, "double-quoted values must be strings")
            return value
        if text.startswith("'"):
            if len(text) < 2 or not text.endswith("'"):
                self.fail(number, "unterminated single-quoted scalar")
            return text[1:-1].replace("''", "'")
        if text in {"true", "false"}:
            return text == "true"
        if text in {"null", "~"}:
            return None
        if INTEGER.fullmatch(text):
            return int(text)
        if text.startswith(("[", "{", "&", "*", "!")):
            self.fail(number, "flow collections, aliases, and tags are outside this subset")
        return text

    def parse(self):
        if not self.lines:
            return {}
        if self.lines[0][0] != 0:
            self.fail(self.lines[0][2], "the document must start at column zero")
        value, index = self.block(0, 0)
        if index != len(self.lines):
            self.fail(self.lines[index][2], "unexpected trailing content")
        return value

    @staticmethod
    def is_list(content):
        return content == "-" or content.startswith("- ")

    def block(self, index, indent):
        if self.is_list(self.lines[index][1]):
            return self.sequence(index, indent)
        return self.mapping(index, indent)

    def mapping(self, index, indent):
        result = {}
        while index < len(self.lines):
            current_indent, content, number = self.lines[index]
            if current_indent < indent:
                break
            if current_indent > indent:
                self.fail(number, "unexpected indentation")
            if self.is_list(content):
                break
            match = PAIR.fullmatch(content)
            if not match:
                self.fail(number, "expected a mapping key")
            key, raw_value = match.groups()
            if key in result:
                self.fail(number, f"duplicate key {key!r}")
            index += 1
            if raw_value is not None:
                result[key] = self.scalar(raw_value, number)
                if index < len(self.lines) and self.lines[index][0] > indent:
                    self.fail(self.lines[index][2], "a scalar cannot have nested content")
                continue
            if index < len(self.lines) and self.lines[index][0] > indent:
                result[key], index = self.block(index, self.lines[index][0])
            else:
                result[key] = {}
        return result, index

    def sequence(self, index, indent):
        result = []
        while index < len(self.lines):
            current_indent, content, number = self.lines[index]
            if current_indent < indent:
                break
            if current_indent > indent:
                self.fail(number, "unexpected sequence indentation")
            if not self.is_list(content):
                break
            item_text = content[1:].lstrip()
            if not item_text:
                index += 1
                if index >= len(self.lines) or self.lines[index][0] <= indent:
                    self.fail(number, "an empty sequence item needs nested content")
                value, index = self.block(index, self.lines[index][0])
                result.append(value)
                continue
            match = PAIR.fullmatch(item_text)
            if not match:
                result.append(self.scalar(item_text, number))
                index += 1
                if index < len(self.lines) and self.lines[index][0] > indent:
                    self.fail(self.lines[index][2], "a scalar item cannot have nested content")
                continue

            key, raw_value = match.groups()
            item = {}
            index += 1
            if raw_value is None:
                if index < len(self.lines) and self.lines[index][0] > indent:
                    item[key], index = self.block(index, self.lines[index][0])
                else:
                    item[key] = {}
            else:
                item[key] = self.scalar(raw_value, number)
            if index < len(self.lines) and self.lines[index][0] == indent + 2:
                rest, index = self.mapping(index, indent + 2)
                duplicates = set(item) & set(rest)
                if duplicates:
                    self.fail(number, f"duplicate sequence mapping keys: {duplicates}")
                item.update(rest)
            if index < len(self.lines) and self.lines[index][0] > indent:
                self.fail(self.lines[index][2], "unexpected nested sequence content")
            result.append(item)
        return result, index


class ObservabilityStackTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        paths = sorted(STACK.rglob("*.yaml")) + sorted(STACK.rglob("*.yml"))
        cls.documents = {
            path.relative_to(STACK).as_posix(): StrictYaml(
                path.read_text(encoding="utf-8"), path
            ).parse()
            for path in paths
        }
        cls.compose = cls.documents["compose.yaml"]
        cls.dashboard = json.loads(
            (STACK / "grafana/dashboards/wukong.json").read_text(encoding="utf-8")
        )
        cls.readme = (STACK / "README.md").read_text(encoding="utf-8")

    def test_official_images_have_exact_fixed_versions(self):
        expected = {
            "collector": "otel/opentelemetry-collector:0.128.0",
            "tempo": "grafana/tempo:2.8.1",
            "loki": "grafana/loki:3.5.1",
            "prometheus": "prom/prometheus:v3.4.1",
            "grafana": "grafana/grafana:12.0.1",
        }
        services = self.compose["services"]
        self.assertEqual(set(services), set(expected))
        for name, image in expected.items():
            with self.subTest(service=name):
                self.assertEqual(services[name]["image"], image)
                self.assertRegex(image.rsplit(":", 1)[1], r"^v?[0-9]+\.[0-9]+\.[0-9]+$")
                self.assertNotIn("latest", image)

    def test_only_expected_loopback_ports_are_published(self):
        expected = {
            "collector": {"4318", "13133"},
            "tempo": {"3200"},
            "loki": {"3100"},
            "prometheus": {"9090"},
            "grafana": {"3000"},
        }
        for name, service in self.compose["services"].items():
            published = set()
            for port in service["ports"]:
                host, host_port, container_port = port.split(":")
                self.assertEqual(host, "127.0.0.1")
                self.assertEqual(host_port, container_port)
                self.assertTrue(container_port.isdigit())
                published.add(host_port)
            self.assertEqual(published, expected[name])

    def test_network_and_container_privileges_are_isolated(self):
        self.assertEqual(self.compose["name"], "wukong-observability")
        self.assertEqual(
            self.compose["networks"],
            {
                "default": {
                    "driver": "bridge",
                    "driver_opts": {"com.docker.network.bridge.enable_ip_masquerade": "false"},
                }
            },
        )
        for name, service in self.compose["services"].items():
            with self.subTest(service=name):
                self.assertNotIn("networks", service)
                self.assertNotIn("network_mode", service)
                self.assertNotIn("privileged", service)
                self.assertEqual(service["cap_drop"], ["ALL"])
                self.assertEqual(service["security_opt"], ["no-new-privileges:true"])

    def test_mounts_are_project_files_or_named_volumes(self):
        expected_state = {
            "tempo": "tempo-data:/var/tempo",
            "loki": "loki-data:/loki",
            "prometheus": "prometheus-data:/prometheus",
            "grafana": "grafana-data:/var/lib/grafana",
        }
        state_mounts = set()
        for name, service in self.compose["services"].items():
            for mount in service["volumes"]:
                self.assertNotRegex(mount.lower(), r"docker\.sock|/var/run/docker|\.\.")
                if mount.startswith("./"):
                    self.assertTrue(mount.endswith(":ro"), mount)
                    self.assertNotRegex(mount.split(":", 1)[0], r"^[A-Za-z]:")
                else:
                    state_mounts.add(mount)
            if name != "collector":
                self.assertIn(expected_state[name], service["volumes"])
        self.assertEqual(state_mounts, set(expected_state.values()))
        self.assertEqual(
            set(self.compose["volumes"]),
            {"tempo-data", "loki-data", "prometheus-data", "grafana-data"},
        )

    def test_no_credentials_or_unpinned_compose_substitutions(self):
        compose_source = (STACK / "compose.yaml").read_text(encoding="utf-8")
        self.assertNotIn("secrets:", self.compose)
        self.assertNotIn("${", compose_source)
        forbidden = ("PASSWORD", "SECRET", "TOKEN", "AUTHORIZATION", "CREDENTIAL")
        environment = self.compose["services"]["grafana"]["environment"]
        for key in environment:
            self.assertFalse(any(word in key.upper() for word in forbidden), key)
        self.assertEqual(environment["GF_AUTH_ANONYMOUS_ENABLED"], "true")
        self.assertEqual(environment["GF_AUTH_ANONYMOUS_ORG_ROLE"], "Viewer")
        self.assertEqual(environment["GF_AUTH_DISABLE_LOGIN_FORM"], "true")
        self.assertEqual(environment["GF_ANALYTICS_REPORTING_ENABLED"], "false")

    def test_collector_uses_standard_signal_exporters_and_complete_pipelines(self):
        collector = self.documents["collector/config.yaml"]
        self.assertEqual(
            collector["receivers"]["otlp"]["protocols"]["http"]["endpoint"],
            "0.0.0.0:4318",
        )
        exporters = collector["exporters"]
        self.assertEqual(set(exporters), {"otlp/tempo", "otlphttp/loki", "prometheus"})
        self.assertEqual(exporters["otlp/tempo"]["endpoint"], "tempo:4317")
        self.assertEqual(exporters["otlphttp/loki"]["endpoint"], "http://loki:3100/otlp")
        self.assertEqual(exporters["prometheus"]["endpoint"], "0.0.0.0:8889")
        expected_exporters = {
            "traces": ["otlp/tempo"],
            "metrics": ["prometheus"],
            "logs": ["otlphttp/loki"],
        }
        pipelines = collector["service"]["pipelines"]
        self.assertEqual(set(pipelines), set(expected_exporters))
        for signal, exporter_names in expected_exporters.items():
            self.assertEqual(pipelines[signal]["receivers"], ["otlp"])
            self.assertEqual(pipelines[signal]["processors"], ["batch"])
            self.assertEqual(pipelines[signal]["exporters"], exporter_names)
        self.assertEqual(collector["service"]["extensions"], ["health_check"])

    def test_backend_storage_scrape_and_retention_are_consistent(self):
        tempo = self.documents["tempo/tempo.yaml"]
        loki = self.documents["loki/loki.yaml"]
        prometheus = self.documents["prometheus/prometheus.yml"]
        self.assertEqual(tempo["storage"]["trace"]["backend"], "local")
        self.assertEqual(tempo["compactor"]["compaction"]["block_retention"], "168h")
        self.assertEqual(loki["limits_config"]["retention_period"], "168h")
        self.assertTrue(loki["limits_config"]["allow_structured_metadata"])
        self.assertEqual(loki["schema_config"]["configs"][0]["schema"], "v13")
        self.assertEqual(
            prometheus["scrape_configs"][0]["static_configs"][0]["targets"],
            ["collector:8889"],
        )
        self.assertIn(
            "--storage.tsdb.retention.time=7d",
            self.compose["services"]["prometheus"]["command"],
        )

    def test_every_service_has_healthchecks_and_dependencies_wait_for_them(self):
        for name, service in self.compose["services"].items():
            with self.subTest(service=name):
                healthcheck = service["healthcheck"]
                self.assertTrue(healthcheck["test"])
                for field in ("interval", "timeout", "retries", "start_period"):
                    self.assertIn(field, healthcheck)
        expected_dependencies = {
            "collector": {"tempo", "loki"},
            "prometheus": {"collector"},
            "grafana": {"tempo", "loki", "prometheus"},
        }
        for name, dependencies in expected_dependencies.items():
            configured = self.compose["services"][name]["depends_on"]
            self.assertEqual(set(configured), dependencies)
            for dependency in dependencies:
                self.assertEqual(configured[dependency]["condition"], "service_healthy")

    def test_grafana_provisioning_and_dashboard_references_resolve(self):
        datasources = self.documents["grafana/provisioning/datasources.yaml"]["datasources"]
        by_uid = {source["uid"]: source for source in datasources}
        self.assertEqual(set(by_uid), {"tempo", "loki", "prometheus"})
        self.assertEqual(by_uid["tempo"]["url"], "http://tempo:3200")
        self.assertEqual(by_uid["loki"]["url"], "http://loki:3100")
        self.assertEqual(by_uid["prometheus"]["url"], "http://prometheus:9090")
        self.assertEqual(by_uid["tempo"]["jsonData"]["tracesToLogsV2"]["datasourceUid"], "loki")
        self.assertEqual(by_uid["loki"]["jsonData"]["derivedFields"][0]["datasourceUid"], "tempo")
        provider = self.documents["grafana/provisioning/dashboards.yaml"]["providers"][0]
        self.assertEqual(provider["options"]["path"], "/var/lib/grafana/dashboards")
        grafana_mounts = self.compose["services"]["grafana"]["volumes"]
        self.assertIn(
            "./grafana/provisioning/datasources.yaml:/etc/grafana/provisioning/datasources/datasources.yaml:ro",
            grafana_mounts,
        )
        self.assertIn(
            "./grafana/provisioning/dashboards.yaml:/etc/grafana/provisioning/dashboards/dashboards.yaml:ro",
            grafana_mounts,
        )
        self.assertIn("./grafana/dashboards:/var/lib/grafana/dashboards:ro", grafana_mounts)

        self.assertEqual(self.dashboard["uid"], "wukong-observability")
        self.assertFalse(self.dashboard["editable"])
        self.assertEqual(
            {variable["name"] for variable in self.dashboard["templating"]["list"]},
            {"run_id", "trace_id"},
        )
        self.assertEqual(
            {panel["type"] for panel in self.dashboard["panels"]},
            {"text", "table", "traces", "logs", "timeseries"},
        )
        for panel in self.dashboard["panels"]:
            if "datasource" in panel:
                self.assertIn(panel["datasource"]["uid"], by_uid)
        dashboard_source = json.dumps(self.dashboard)
        self.assertIn("${run_id}", dashboard_source)
        self.assertIn("$trace_id", dashboard_source)
        self.assertEqual(self.dashboard["links"][0]["url"], "/explore")
        self.assertEqual(
            self.dashboard["panels"][0]["pluginVersion"],
            self.compose["services"]["grafana"]["image"].rsplit(":", 1)[1],
        )

    def test_readme_has_task_scoped_lifecycle_health_and_limits(self):
        prefix = "docker compose -f scripts/observability/compose.yaml"
        self.assertIn(f"{prefix} config", self.readme)
        self.assertIn(f"{prefix} up -d --wait", self.readme)
        self.assertIn(f"{prefix} ps", self.readme)
        self.assertIn(f"{prefix} down", self.readme)
        self.assertNotIn("prune", self.readme.lower())
        for endpoint in (
            "127.0.0.1:4318",
            "127.0.0.1:13133",
            "127.0.0.1:3200/ready",
            "127.0.0.1:3100/ready",
            "127.0.0.1:9090/-/ready",
            "127.0.0.1:3000/api/health",
        ):
            self.assertIn(endpoint, self.readme)
        self.assertIn("do not replace real OTLP", self.readme)

    def test_acceptance_probe_is_loopback_only_and_task_scoped(self):
        source = (STACK / "acceptance.py").read_text(encoding="utf-8")
        ast.parse(source, filename="acceptance.py")
        namespace = {}
        for node in ast.parse(source).body:
            if isinstance(node, ast.Assign) and len(node.targets) == 1:
                target = node.targets[0]
                if isinstance(target, ast.Name) and target.id in {
                    "COLLECTOR",
                    "TEMPO",
                    "LOKI",
                    "PROMETHEUS",
                    "GRAFANA",
                }:
                    namespace[target.id] = ast.literal_eval(node.value)
        self.assertEqual(set(namespace), {"COLLECTOR", "TEMPO", "LOKI", "PROMETHEUS", "GRAFANA"})
        for endpoint in namespace.values():
            self.assertTrue(endpoint.startswith("http://127.0.0.1:"), endpoint)
        self.assertIn("python -I scripts/observability/acceptance.py", self.readme)
        self.assertIn("not_covered", source)
        self.assertNotIn('add_argument("--endpoint"', source)


if __name__ == "__main__":
    unittest.main()
