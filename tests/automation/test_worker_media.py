"""Exercise reproducible offline worker media staging, finalization and checks."""

import base64
import errno
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

from scripts._lib.checksums import sha256
from scripts.worker import media

REVISION = "a" * 40
WORKER = b"worker fixture"
PACKAGE = b"package fixture"
PUBLIC_KEY = "c" * 64
KEY_SHA256 = "d" * 64
SSH_BLOB = b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20" + b"k" * 32
SSH_PUBLIC = dict(algorithm="ssh-ed25519", key=base64.b64encode(SSH_BLOB).decode("ascii"))
# Mirrors the deepest shipped payload member (131 characters) from the Windows
# Kits debugger samples; a POSIX test filesystem accepts the real path, so the
# Windows MAX_PATH failure is emulated explicitly in the deep-output test.
LONG_MEMBER = (
    "runtime/sdk/Windows Kits/10/Debuggers/x64/sdk/samples/exdi/"
    "ExdiGdbSrvSample/GdbSrvControllerLib/GdbSrvControllerLib.vcxproj.filters"
)
LONG_BYTES = b"long payload fixture"
SPEC = dict(
    schema=1,
    installation_id="b" * 32,
    environment_id="lab",
    generation=2,
    management_peer="192.0.2.20",
    ssh_port=2222,
    ssh_public_key=SSH_PUBLIC,
)


def digest(value):
    return hashlib.sha256(value).hexdigest()


def native_metadata():
    return dict(
        schema=1,
        platform="windows-amd64",
        source_revision=REVISION,
        files={
            "wukong-worker.exe": dict(sha256=digest(WORKER), size=len(WORKER)),
            "wukong-package.exe": dict(sha256=digest(PACKAGE), size=len(PACKAGE)),
        },
    )


def manifest_draft(native_bytes, lock, long_member=False):
    files = {
        "native-build.json": dict(sha256=digest(native_bytes), size=len(native_bytes), kind="data"),
        "runtime/native/wukong-worker.exe": dict(
            sha256=digest(WORKER), size=len(WORKER), kind="runtime"
        ),
        "toolchain.lock.json": dict(sha256=digest(lock), size=len(lock), kind="data"),
    }
    if long_member:
        files[LONG_MEMBER] = dict(sha256=digest(LONG_BYTES), size=len(LONG_BYTES), kind="runtime")
    return dict(
        schema=2,
        platform="windows-amd64",
        version="1.0.0",
        source_revision=REVISION,
        toolchain_lock_sha256=digest(lock),
        toolchain_lock_path="toolchain.lock.json",
        files=files,
    )


def build_template(root, long_member=False):
    root = Path(root)
    payload = root / "payload"
    (payload / "runtime/native").mkdir(parents=True)
    lock = b"toolchain lock fixture"
    native = dict(native_metadata())
    native_bytes = json.dumps(native).encode()
    (payload / "toolchain.lock.json").write_bytes(lock)
    (payload / "native-build.json").write_bytes(native_bytes)
    (payload / "runtime/native/wukong-worker.exe").write_bytes(WORKER)
    if long_member:
        (payload / LONG_MEMBER).parent.mkdir(parents=True)
        (payload / LONG_MEMBER).write_bytes(LONG_BYTES)
    (root / "native-build.json").write_bytes(native_bytes)
    binaries = root / "binaries"
    binaries.mkdir()
    (binaries / "wukong-worker.exe").write_bytes(WORKER)
    (binaries / "wukong-package.exe").write_bytes(PACKAGE)
    manifest = manifest_draft(native_bytes, lock, long_member)
    (root / "manifest-draft.json").write_text(json.dumps(manifest), encoding="utf-8")
    record = dict(
        schema=1,
        version="1.0.0",
        source_revision=REVISION,
        payload_manifest_draft_sha256=digest(json.dumps(manifest).encode()),
        binaries={
            name: dict(
                sha256=digest((binaries / name).read_bytes()), size=(binaries / name).stat().st_size
            )
            for name in media.BINARIES
        },
        toolchain_lock_sha256=digest(lock),
    )
    (root / "media-template.json").write_text(json.dumps(record), encoding="utf-8")
    return root


def build_payload(output, binaries, long_member=False):
    output = Path(output)
    (output / "runtime/native").mkdir(parents=True)
    lock = b"toolchain lock fixture"
    native = dict(native_metadata())
    native_bytes = json.dumps(native).encode()
    (output / "toolchain.lock.json").write_bytes(lock)
    (output / "native-build.json").write_bytes(native_bytes)
    (output / "runtime/native/wukong-worker.exe").write_bytes(WORKER)
    if long_member:
        (output / LONG_MEMBER).parent.mkdir(parents=True)
        (output / LONG_MEMBER).write_bytes(LONG_BYTES)
    binaries.mkdir(parents=True)
    (binaries / "wukong-worker.exe").write_bytes(WORKER)
    (binaries / "wukong-package.exe").write_bytes(PACKAGE)
    return manifest_draft(native_bytes, lock, long_member), native


def deep_output(parent, length=100):
    """Return a media output whose parent path is exactly ``length`` characters.

    With a 100-character parent the old output-parent workspace would stage the
    131-character payload member beyond Windows' 259-character limit, while the
    final media output itself still fits.
    """
    parent = Path(parent)
    remaining = length - len(str(parent)) - 1
    if remaining < 1:
        raise AssertionError("temporary root is already too deep for this fixture")
    return parent / ("d" * remaining) / "media"


class TemplateTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        # Keep workspace selection deterministic on hosts whose TEMP path is
        # longer than the production budget; WorkspaceTests covers that budget.
        self.workspace_budget = patch.object(media, "STAGED_PATH_BUDGET", 10_000)
        self.workspace_budget.start()
        self.addCleanup(self.workspace_budget.stop)
        self.workspace_root = Path(tempfile.gettempdir())

    def tearDown(self):
        self.temporary.cleanup()

    def assert_no_workspace_leak(self):
        self.assertEqual(list(self.workspace_root.glob(".worker-media-template-*")), [])

    def test_template_failure_leaves_no_output_or_staging(self):
        output = self.root / "template"
        with (
            patch.object(media.platform, "system", return_value="Windows"),
            patch.object(media.platform, "machine", return_value="AMD64"),
            patch.object(media, "version", return_value="1.0.0"),
            patch.object(media, "stage", side_effect=ValueError("synthetic failure")),
        ):
            with self.assertRaisesRegex(ValueError, "synthetic failure"):
                media.template(self.root / "cache", self.root / "native", output)
        self.assertFalse(output.exists())
        self.assert_no_workspace_leak()

    def test_template_stages_unsigned_payload_binaries_and_record(self):
        output = self.root / "template"

        def fake_stage(cache, staged, native):
            manifest, native_metadata_value = build_payload(staged, self.root / "native-build")
            return dict(
                manifest=manifest,
                binaries=self.root / "native-build",
                native_build=native_metadata_value,
            )

        with (
            patch.object(media.platform, "system", return_value="Windows"),
            patch.object(media.platform, "machine", return_value="AMD64"),
            patch.object(media, "version", return_value="1.0.0"),
            patch.object(media, "stage", side_effect=fake_stage),
        ):
            result = media.template(self.root / "cache", self.root / "native", output, "1.0.0")
        self.assertTrue(result["prepared"])
        self.assertEqual(result["source_revision"], REVISION)
        loaded = media._load_template(output)
        self.assertEqual(loaded["record"]["version"], "1.0.0")
        self.assertEqual(loaded["record"]["source_revision"], REVISION)
        self.assertEqual(
            sorted(path.name for path in output.iterdir()),
            [
                "binaries",
                "manifest-draft.json",
                "media-template.json",
                "native-build.json",
                "payload",
            ],
        )
        self.assert_no_workspace_leak()
        with (
            patch.object(media.platform, "system", return_value="Windows"),
            patch.object(media.platform, "machine", return_value="AMD64"),
            patch.object(media, "version", return_value="1.0.0"),
        ):
            with self.assertRaisesRegex(ValueError, "already exists"):
                media.template(self.root / "cache", self.root / "native", output)

    def test_template_rejects_too_deep_output_before_staging(self):
        output = self.root / ("t" * 150)
        with (
            patch.object(media.platform, "system", return_value="Windows"),
            patch.object(media.platform, "machine", return_value="AMD64"),
            patch.object(media, "version", return_value="1.0.0"),
            patch.object(media, "stage") as stage,
        ):
            with self.assertRaisesRegex(ValueError, "too long for Windows MAX_PATH"):
                media.template(self.root / "cache", self.root / "native", output)
        stage.assert_not_called()
        self.assertFalse(output.exists())
        self.assert_no_workspace_leak()

    def test_pack_is_deterministic_and_refuses_overwrite(self):
        template = build_template(self.root / "template")
        first = self.root / "first.zip"
        second = self.root / "second.zip"
        media.pack(template, "1.0.0", first)
        media.pack(template, "1.0.0", second)
        self.assertEqual(first.read_bytes(), second.read_bytes())
        with zipfile.ZipFile(self.root / "first.zip") as archive:
            names = archive.namelist()
            self.assertEqual(names, sorted(names))
            self.assertTrue(names)
            self.assertTrue(
                all(name.startswith("wukong-windows-worker-media-1.0.0/") for name in names)
            )
            self.assertTrue(
                all(not name.startswith(("/", "\\")) and ":" not in name for name in names)
            )
            self.assertTrue(
                all(item.date_time == (1980, 1, 1, 0, 0, 0) for item in archive.infolist())
            )
        with self.assertRaisesRegex(ValueError, "already exists"):
            media.pack(template, "1.0.0", first)
        with self.assertRaisesRegex(ValueError, "differs"):
            media.pack(template, "2.0.0", self.root / "other.zip")

    def test_pack_accepts_a_directory_output(self):
        template = build_template(self.root / "template")
        result = media.pack(template, "1.0.0", self.root / "dist")
        self.assertEqual(Path(result["archive"]).name, "wukong-windows-worker-media-1.0.0.zip")
        self.assertTrue(Path(result["archive"]).is_file())

    def test_publication_retries_transient_windows_file_locks(self):
        source = self.root / "source"
        source.mkdir()
        (source / "payload").write_bytes(b"fixture")
        target = self.root / "target"
        real_replace = os.replace
        calls = []

        def replace(first, second):
            calls.append((first, second))
            if len(calls) == 1:
                raise OSError(32, "The process cannot access the file")
            return real_replace(first, second)

        with (
            patch.object(media.os, "replace", side_effect=replace),
            patch.object(media.time, "sleep") as sleep,
        ):
            media._publish_directory(source, target)
        self.assertEqual(len(calls), 2)
        sleep.assert_called_once()
        self.assertTrue((target / "payload").is_file())
        self.assertFalse(source.exists())

    def test_publication_fails_without_overwriting_after_real_errors(self):
        source = self.root / "source"
        source.mkdir()
        (source / "payload").write_bytes(b"fixture")
        target = self.root / "target"
        with (
            patch.object(
                media.os,
                "replace",
                side_effect=OSError(32, "The process cannot access the file"),
            ),
            patch.object(media.time, "sleep"),
        ):
            with self.assertRaisesRegex(ValueError, "could not be published"):
                media._publish_directory(source, target)
        self.assertTrue(source.exists())
        self.assertFalse(target.exists())

    def test_publication_copies_between_volumes(self):
        source = self.root / "source"
        (source / "payload").mkdir(parents=True)
        (source / "payload" / "data.bin").write_bytes(b"fixture")
        target = self.root / "target"
        with patch.object(media.os, "replace", side_effect=OSError(errno.EXDEV, "cross-volume")):
            media._publish_directory(source, target)
        self.assertEqual((target / "payload" / "data.bin").read_bytes(), b"fixture")
        self.assertFalse(source.exists())

    def test_publication_removes_partial_cross_volume_copy_on_failure(self):
        source = self.root / "source"
        source.mkdir()
        (source / "payload").write_bytes(b"fixture")
        target = self.root / "target"

        def partial_copy(from_path, to_path):
            Path(to_path).mkdir()
            (Path(to_path) / "partial.bin").write_bytes(b"partial")
            raise OSError(13, "locked fixture")

        with (
            patch.object(media.os, "replace", side_effect=OSError(errno.EXDEV, "cross-volume")),
            patch.object(media, "_copy_tree", side_effect=partial_copy),
            patch.object(media.time, "sleep"),
        ):
            with self.assertRaisesRegex(ValueError, "could not be published"):
                media._publish_directory(source, target)
        self.assertTrue(source.exists())
        self.assertFalse(target.exists())


class FinalizeTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        # Keep workspace selection deterministic on hosts whose TEMP path is
        # longer than the production budget; WorkspaceTests covers that budget.
        self.workspace_budget = patch.object(media, "STAGED_PATH_BUDGET", 10_000)
        self.workspace_budget.start()
        self.addCleanup(self.workspace_budget.stop)
        self.workspace_root = Path(tempfile.gettempdir())
        self.template = build_template(self.root / "template")
        self.keys = self.root / "keys"
        self.keys.mkdir()
        (self.keys / "signing-key.bin").write_bytes(b"private key fixture")
        self.spec = self.root / "spec.json"
        self.spec.write_text(json.dumps(SPEC), encoding="utf-8")
        self.commands = []
        self.manifest_sha = None

    def tearDown(self):
        self.temporary.cleanup()

    def assert_no_workspace_leak(self):
        self.assertEqual(list(self.workspace_root.glob(".worker-media-finalize-*")), [])

    def fake_run(self, command, **kwargs):
        self.commands.append([str(item) for item in command])
        if command[1] == "key":
            return SimpleNamespace(
                stdout=json.dumps(dict(public_key=PUBLIC_KEY, key_sha256=KEY_SHA256))
            )
        if command[1] == "seal":
            manifest = json.dumps(dict(version=command[5])).encode()
            (Path(command[2]) / "manifest.json").write_bytes(manifest)
            (Path(command[2]) / "manifest.sig").write_bytes(b"sealed signature")
            self.manifest_sha = digest(manifest)
            return SimpleNamespace(
                stdout=json.dumps(
                    dict(sealed=True, manifest_sha256=self.manifest_sha, key_sha256=KEY_SHA256)
                )
            )
        if command[1] == "enroll":
            enrollment = Path(command[5])
            enrollment.mkdir()
            (enrollment / "enrollment.json").write_bytes(b"enrollment record")
            (enrollment / "enrollment.sig").write_bytes(b"enrollment signature")
            return SimpleNamespace(
                stdout=json.dumps(
                    dict(
                        enrollment_prepared=True,
                        installation_id=SPEC["installation_id"],
                        ssh_key_fingerprint="SHA256:fixture",
                    )
                )
            )
        raise AssertionError("unexpected command: " + repr(command))

    def fake_prepare(
        self,
        package,
        enrollment,
        output,
        key,
        controller,
        verifier,
        previous_package=None,
        previous_enrollment=None,
    ):
        output.mkdir()
        shutil.copytree(package, output / "payload")
        (output / "enrollment").mkdir()
        for name in ("enrollment.json", "enrollment.sig"):
            shutil.copyfile(enrollment / name, output / "enrollment" / name)
        shutil.copyfile(verifier, output / "WukongWorkerSetup.exe")
        (output / "setup.json").write_text(
            json.dumps(dict(schema=1, controller_public_key=key, controller_version=controller)),
            encoding="utf-8",
        )
        return dict(
            prepared=True,
            scope="fixture",
            installation_id=SPEC["installation_id"],
            manifest_sha256=self.manifest_sha,
            setup_sha256=digest(WORKER),
            installed=False,
        )

    def test_finalize_orders_key_seal_enroll_and_writes_receipt(self):
        output = self.root / "media"
        with (
            patch.object(media.subprocess, "run", side_effect=self.fake_run),
            patch.object(media, "prepare_bundle", side_effect=self.fake_prepare),
        ):
            record = media.finalize(self.template, self.keys, self.spec, output)
        self.assertEqual([command[1] for command in self.commands], ["key", "seal", "enroll"])
        self.assertEqual(self.commands[0][2], str(self.keys))
        self.assertEqual(self.commands[1][3], str(self.template / "manifest-draft.json"))
        self.assertEqual(self.commands[1][4], str(self.keys))
        self.assertEqual(self.commands[1][5], "1.0.0")
        self.assertEqual(self.commands[2][4], str(self.keys))
        self.assertEqual(self.commands[2][6], "1.0.0")
        self.assertEqual(record["version"], "1.0.0")
        self.assertEqual(record["controller_public_key"], PUBLIC_KEY)
        self.assertEqual(record["manifest_sha256"], self.manifest_sha)
        self.assertEqual(record["setup_sha256"], digest(WORKER))
        self.assertEqual(record["enrollment_sha256"], digest(b"enrollment record"))
        self.assertEqual(record["source_revision"], REVISION)
        self.assertEqual(json.loads((output / "media.json").read_text(encoding="utf-8")), record)
        self.assertEqual(sha256(output / "WukongWorkerSetup.exe"), record["setup_sha256"])
        self.assertEqual(
            sorted(path.name for path in output.iterdir()),
            ["WukongWorkerSetup.exe", "enrollment", "media.json", "payload", "setup.json"],
        )
        names = [path.name for path in output.rglob("*") if path.is_file()]
        self.assertNotIn("signing-key.bin", names)

    def test_finalize_fails_closed_when_keys_or_generated_key_stay_inside_media(self):
        output = self.root / "media"
        for keys in [output / "keys", self.template / "keys"]:
            with self.subTest(keys=str(keys)):
                with self.assertRaisesRegex(ValueError, "must stay outside"):
                    media.finalize(self.template, keys, self.spec, output)
                self.assertFalse(output.exists())

    def test_finalize_requires_one_ssh_key_source(self):
        generated = self.root / "management-ed25519"
        with (
            patch.object(media.subprocess, "run", side_effect=self.fake_run),
            self.assertRaisesRegex(ValueError, "either"),
        ):
            media.finalize(
                self.template,
                self.keys,
                self.spec,
                self.root / "media",
                generate_ssh_key=generated,
            )
        plain = self.root / "plain.json"
        plain.write_text(
            json.dumps({key: value for key, value in SPEC.items() if key != "ssh_public_key"}),
            encoding="utf-8",
        )
        with self.assertRaisesRegex(ValueError, "must include"):
            media.finalize(self.template, self.keys, plain, self.root / "other")

    def test_finalize_requires_both_recovery_sources(self):
        with self.assertRaisesRegex(ValueError, "both previous"):
            media.finalize(
                self.template,
                self.keys,
                self.spec,
                self.root / "media",
                previous_template=self.template,
            )

    def emulated_max_path_copy(self, real_copy_tree):
        """Fail like Windows when a copy destination passes MAX_PATH.

        POSIX test filesystems accept arbitrarily long paths, so the copy tool
        is wrapped to reproduce the real Windows failure deterministically.
        """

        def copy(source, destination):
            source, destination = Path(source), Path(destination)
            for current, directories, files in os.walk(source):
                relative = Path(current).relative_to(source)
                for name in [*directories, *files]:
                    candidate = destination / relative / name
                    if len(str(candidate)) > media.WINDOWS_PATH_LIMIT:
                        raise OSError(2, "No such file or directory", str(candidate))
            return real_copy_tree(source, destination)

        return copy

    def test_finalize_uses_short_workspace_for_deep_output(self):
        # Regression: the old finalize staged below the deep output parent, so
        # the 131-character payload member crossed MAX_PATH during copying.
        template = build_template(self.root / "long-template", long_member=True)
        output = deep_output(self.root)
        deep_parent = output.parent
        real_copy_tree = media._copy_tree
        with (
            patch.object(media.subprocess, "run", side_effect=self.fake_run),
            patch.object(media, "prepare_bundle", side_effect=self.fake_prepare),
            patch.object(
                media, "_copy_tree", side_effect=self.emulated_max_path_copy(real_copy_tree)
            ),
        ):
            with patch.object(media, "_workspace_base", return_value=deep_parent):
                with self.assertRaises(OSError):
                    media.finalize(template, self.keys, self.spec, output)
            self.assertFalse(output.exists())
            self.assertEqual(list(deep_parent.glob(".worker-media-finalize-*")), [])
            self.assert_no_workspace_leak()
            record = media.finalize(template, self.keys, self.spec, output)
        self.assertEqual(json.loads((output / "media.json").read_text(encoding="utf-8")), record)
        self.assertEqual((output / "payload" / LONG_MEMBER).read_bytes(), LONG_BYTES)
        self.assert_no_workspace_leak()

    def test_finalize_rejects_too_deep_output_before_copying(self):
        template = build_template(self.root / "long-template", long_member=True)
        output = deep_output(self.root, length=150)
        with (
            patch.object(media.subprocess, "run") as run,
            patch.object(media, "_copy_tree") as copy_tree,
        ):
            with self.assertRaisesRegex(ValueError, "too long for Windows MAX_PATH") as raised:
                media.finalize(template, self.keys, self.spec, output)
        message = str(raised.exception)
        self.assertIn("state_dir", message)
        self.assertIn("may be at most", message)
        run.assert_not_called()
        copy_tree.assert_not_called()
        self.assertFalse(output.exists())
        self.assert_no_workspace_leak()


class SpecTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()

    def tearDown(self):
        self.temporary.cleanup()

    def read(self, value):
        path = self.root / "spec.json"
        path.write_text(json.dumps(value), encoding="utf-8")
        return media.read_spec(path)

    def test_valid_spec_defaults_generation_and_boundary(self):
        value = {
            key: item for key, item in SPEC.items() if key not in {"ssh_public_key", "generation"}
        }
        result = self.read(value)
        self.assertEqual(result["generation"], 1)
        self.assertEqual(result["execution_boundary"], "disposable_vm")
        self.assertIsNone(result["ssh_public_key"])
        result = self.read(SPEC)
        self.assertEqual(result["ssh_public_key"], SSH_PUBLIC)
        self.assertEqual(result["generation"], 2)

    def test_invalid_specs_are_rejected(self):
        cases = [
            dict(SPEC, schema=2),
            dict(SPEC, extra="field"),
            dict(SPEC, installation_id="B" * 32),
            dict(SPEC, installation_id="b" * 31),
            dict(SPEC, installation_id=32),
            dict(SPEC, environment_id=""),
            dict(SPEC, environment_id="bad environment"),
            dict(SPEC, environment_id="x" * 65),
            dict(SPEC, generation=0),
            dict(SPEC, generation=True),
            dict(SPEC, management_peer="0.0.0.0"),
            dict(SPEC, management_peer="127.0.0.1"),
            dict(SPEC, management_peer="224.0.0.1"),
            dict(SPEC, management_peer="255.255.255.255"),
            dict(SPEC, management_peer="::1"),
            dict(SPEC, management_peer="192.0.2.020"),
            dict(SPEC, management_peer=192),
            dict(SPEC, ssh_port=0),
            dict(SPEC, ssh_port=65536),
            dict(SPEC, ssh_port=True),
            dict(SPEC, execution_boundary="trusted_local"),
            dict(SPEC, ssh_public_key=dict(algorithm="ssh-rsa", key=SSH_PUBLIC["key"])),
            dict(SPEC, ssh_public_key=dict(algorithm="ssh-ed25519", key="AAAA")),
            dict(SPEC, ssh_public_key=dict(SSH_PUBLIC, comment="extra")),
        ]
        for case in cases:
            with self.subTest(case=json.dumps(case)[:120]):
                with self.assertRaises(ValueError):
                    self.read(case)

    def test_read_spec_rejects_oversized_or_non_object_input(self):
        path = self.root / "spec.json"
        path.write_text("[" + " " * (64 * 1024) + "]", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "64 KiB"):
            media.read_spec(path)
        path.write_text("[]", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "Unsupported"):
            media.read_spec(path)


class WorkspaceTests(unittest.TestCase):
    """Workspace base selection keeps staged media paths inside Windows limits."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        self.output_parent = self.root / "output-parent"
        self.output_parent.mkdir()

    def tearDown(self):
        self.temporary.cleanup()

    def directory(self, name):
        path = self.root / name
        path.mkdir()
        return path

    def test_prefers_temp_over_tmp_system_temp_and_output_parent(self):
        spec = patch.object(media, "STAGED_PATH_BUDGET", 10_000)
        temp = self.directory("temp")
        tmp = self.directory("tmp")
        with (
            patch.dict(media.os.environ, {"TEMP": str(temp), "TMP": str(tmp)}, clear=True),
            patch.object(media.tempfile, "gettempdir", return_value=str(tmp)),
            spec,
        ):
            self.assertEqual(media._workspace_base(self.output_parent), temp)

    def test_falls_back_to_system_temp_and_output_parent(self):
        spec = patch.object(media, "STAGED_PATH_BUDGET", 10_000)
        system_temp = self.directory("system-temp")
        missing = self.root / "missing-temp"
        with (
            patch.dict(media.os.environ, {"TEMP": str(missing)}, clear=True),
            patch.object(media.tempfile, "gettempdir", return_value=str(system_temp)),
            spec,
        ):
            self.assertEqual(media._workspace_base(self.output_parent), system_temp)
            with patch.object(media.tempfile, "gettempdir", return_value=str(missing)):
                self.assertEqual(media._workspace_base(self.output_parent), self.output_parent)

    def test_rejects_too_long_candidates_with_actionable_error(self):
        too_long = str(self.root / ("x" * 200))
        with (
            patch.dict(media.os.environ, {"TEMP": too_long, "TMP": too_long}, clear=True),
            patch.object(media.tempfile, "gettempdir", return_value=too_long),
        ):
            with self.assertRaisesRegex(ValueError, "needs a shorter") as raised:
                media._workspace_base(Path(too_long))
        message = str(raised.exception)
        self.assertIn("state_dir", message)
        self.assertIn("shorter", message)
        self.assertIn("at most 41", message)

    def test_workspace_creates_and_removes_a_named_directory(self):
        temp = self.directory("temp")
        with (
            patch.dict(media.os.environ, {"TEMP": str(temp)}, clear=True),
            patch.object(media, "STAGED_PATH_BUDGET", 10_000),
        ):
            workspace = media._workspace(self.output_parent, ".worker-media-test-")
            try:
                self.assertTrue(workspace.is_dir())
                self.assertEqual(workspace.parent, temp)
                self.assertTrue(workspace.name.startswith(".worker-media-test-"))
            finally:
                workspace.rmdir()
        self.assertEqual(list(temp.glob(".worker-media-test-*")), [])

    def test_workspace_cleanup_retries_transient_file_locks(self):
        workspace = self.directory("workspace")
        (workspace / "payload").write_bytes(b"fixture")
        real_rmtree = shutil.rmtree
        calls = []

        def rmtree(path, ignore_errors=False):
            calls.append(path)
            if len(calls) < 3:
                (Path(path) / "payload").unlink(missing_ok=True)
                (Path(path) / "payload").write_bytes(b"locked fixture")
                return
            return real_rmtree(path, ignore_errors=ignore_errors)

        with (
            patch.object(media.shutil, "rmtree", side_effect=rmtree),
            patch.object(media.time, "sleep") as sleep,
        ):
            media._discard_workspace(workspace)
        self.assertEqual(len(calls), 3)
        self.assertFalse(workspace.exists())
        self.assertEqual(sleep.call_count, 2)


class CheckTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        # Keep workspace selection deterministic on hosts whose TEMP path is
        # longer than the production budget; WorkspaceTests covers that budget.
        self.workspace_budget = patch.object(media, "STAGED_PATH_BUDGET", 10_000)
        self.workspace_budget.start()
        self.addCleanup(self.workspace_budget.stop)
        self.template = self.root / "template"
        binaries = self.template / "binaries"
        binaries.mkdir(parents=True)
        (binaries / "wukong-worker.exe").write_bytes(WORKER)
        self.stub = dict(
            path=self.template,
            binaries=binaries,
            record=dict(
                version="1.0.0",
                binaries={"wukong-worker.exe": dict(sha256=digest(WORKER), size=len(WORKER))},
            ),
            manifest=dict(source_revision=REVISION),
        )
        self.state = {}

    def tearDown(self):
        self.temporary.cleanup()

    def fake_finalize(self, setup_bytes):
        def finalize(
            template_path,
            keys,
            spec_path,
            output,
            controller_version=None,
            generate_ssh_key=None,
            previous_template=None,
            previous_enrollment=None,
        ):
            output.mkdir()
            payload = output / "payload"
            payload.mkdir()
            (payload / "manifest.json").write_bytes(b"sealed manifest")
            (payload / "manifest.sig").write_bytes(b"sealed signature")
            enrollment = output / "enrollment"
            enrollment.mkdir()
            (enrollment / "enrollment.json").write_bytes(b"enrollment record")
            (enrollment / "enrollment.sig").write_bytes(b"enrollment signature")
            (output / "WukongWorkerSetup.exe").write_bytes(setup_bytes)
            (output / "setup.json").write_text(
                json.dumps(
                    dict(
                        schema=1,
                        controller_public_key=PUBLIC_KEY,
                        controller_version="1.0.0",
                    )
                ),
                encoding="utf-8",
            )
            record = dict(
                schema=1,
                version="1.0.0",
                controller_public_key=PUBLIC_KEY,
                manifest_sha256=digest(b"sealed manifest"),
                setup_sha256=digest(setup_bytes),
                enrollment_sha256=digest(b"enrollment record"),
                installation_id="b" * 32,
                environment_id="lab",
                generation=1,
                source_revision=REVISION,
            )
            (output / "media.json").write_text(json.dumps(record), encoding="utf-8")
            self.state["record"] = record
            return record

        return finalize

    def fake_verify(self, verified):
        def run(command, **kwargs):
            self.assertEqual(command[1], "verify-enrollment")
            self.assertEqual(len(command), 6)
            return SimpleNamespace(
                stdout=json.dumps(
                    dict(
                        verified=verified,
                        execution_boundary="disposable_vm",
                        installation_id=self.state["record"]["installation_id"],
                        manifest_sha256=self.state["record"]["manifest_sha256"],
                    )
                )
            )

        return run

    def test_check_accepts_ephemeral_finalized_media_and_writes_report(self):
        report_path = self.root / "report.json"
        with (
            patch.object(media, "_load_template", return_value=self.stub),
            patch.object(media, "finalize", side_effect=self.fake_finalize(WORKER)),
            patch.object(media.subprocess, "run", side_effect=self.fake_verify(True)),
        ):
            report = media.check(self.template, report_path)
        self.assertTrue(report["passed"])
        self.assertTrue(report["verified"])
        self.assertEqual(json.loads(report_path.read_text(encoding="utf-8")), report)
        self.assertEqual(report["template"], str(self.template))

    def test_check_rejects_setup_executable_differing_from_template_worker(self):
        report_path = self.root / "report.json"
        with (
            patch.object(media, "_load_template", return_value=self.stub),
            patch.object(media, "finalize", side_effect=self.fake_finalize(b"tampered setup")),
            patch.object(media.subprocess, "run", side_effect=self.fake_verify(True)),
        ):
            with self.assertRaisesRegex(ValueError, "template worker"):
                media.check(self.template, report_path)
        self.assertFalse(report_path.exists())

    def test_check_rejects_unverified_enrollment_evidence(self):
        report_path = self.root / "report.json"
        with (
            patch.object(media, "_load_template", return_value=self.stub),
            patch.object(media, "finalize", side_effect=self.fake_finalize(WORKER)),
            patch.object(media.subprocess, "run", side_effect=self.fake_verify(False)),
        ):
            with self.assertRaisesRegex(ValueError, "did not accept"):
                media.check(self.template, report_path)
        self.assertFalse(report_path.exists())


class IsoTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        self.media = self.root / "media"
        self.media.mkdir()
        self.mkisofs = self.root / "mkisofs.exe"
        self.mkisofs.write_bytes(b"fixture tool")
        self.output = self.root / "offline.iso"
        self.commands = []

    def tearDown(self):
        self.temporary.cleanup()

    def test_iso_uses_fixed_arguments(self):
        def run(command, **kwargs):
            self.commands.append(command)
            Path(command[command.index("-o") + 1]).write_bytes(b"iso image")
            return SimpleNamespace(returncode=0)

        with patch.object(media.subprocess, "run", side_effect=run):
            result = media.iso(self.media, self.mkisofs, self.output)
        self.assertEqual(
            self.commands,
            [
                [
                    str(self.mkisofs),
                    "-quiet",
                    "-iso-level",
                    "4",
                    "-joliet",
                    "-joliet-long",
                    "-V",
                    "WUKONG_OFFLINE",
                    "-o",
                    str(self.output),
                    str(self.media),
                ]
            ],
        )
        self.assertEqual(result["label"], "WUKONG_OFFLINE")
        self.assertEqual(result["size"], len(b"iso image"))

    def test_iso_refuses_existing_output_relative_tool_and_failures(self):
        self.output.write_bytes(b"existing")
        with self.assertRaisesRegex(ValueError, "already exists"):
            media.iso(self.media, self.mkisofs, self.output)
        with self.assertRaisesRegex(ValueError, "absolute"):
            media.iso(self.media, Path("mkisofs.exe"), self.root / "other.iso")
        with patch.object(
            media.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "mkisofs")
        ):
            with self.assertRaises(subprocess.CalledProcessError):
                media.iso(self.media, self.mkisofs, self.root / "other.iso")


if __name__ == "__main__":
    unittest.main()
