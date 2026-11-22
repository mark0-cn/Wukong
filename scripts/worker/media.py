"""Build, finalize, check and package reproducible offline Windows worker media.

`template` runs in CI without controller keys: it stages the complete payload,
copies the authenticated native binaries and records the unsigned manifest
draft. `finalize` runs only on the operator controller: it seals the payload
with the controller key directory, signs enrollment and bundles the setup media
whose setup program is named `WukongWorkerSetup.exe`. `check` exercises
finalize with an ephemeral key and verifies the copied media without elevation.
`pack` writes a deterministic release archive and `iso` wraps a finalized media
directory. No command installs anything.
"""

import argparse
import base64
import errno
import ipaddress
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tempfile
import time
import uuid
import zipfile

from scripts._lib.checksums import sha256
from scripts._lib.repository import ROOT
from scripts._lib.versioning import VERSION, version
from scripts.worker.assemble import BINARIES, stage
from scripts.worker.bundle import prepare as prepare_bundle
from scripts.worker.paths import member, no_links, regular

TEMPLATE_SCHEMA = 1
MEDIA_SCHEMA = 1
ARCHIVE_ROOT = "wukong-windows-worker-media"
ALLOWLIST = frozenset(
    {"WukongWorkerSetup.exe", "setup.json", "payload", "enrollment", "media.json"}
)
RECOVERY = "recovery"
HASH = re.compile(r"[0-9a-f]{64}")
SPEC_FIELDS = frozenset(
    {
        "schema",
        "installation_id",
        "environment_id",
        "generation",
        "management_peer",
        "ssh_port",
        "execution_boundary",
        "ssh_public_key",
    }
)
SSH_ED25519_PREFIX = b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20"

# Windows MAX_PATH is 260 characters including the terminating NUL, so an
# accessible path has at most 259 characters unless LongPathsEnabled=1. The
# offline payload nests up to two payload copies below one staging workspace:
#   <base>/<workspace>/media/payload/<relative member>
# so the workspace base and the final output need explicit length budgets.
WINDOWS_PATH_LIMIT = 259
# Deepest essential payload member: 131 characters under
# runtime/sdk/Windows Kits/10/Debuggers/... (debugger samples); 132 rounds up
# the OS separator that joins it to its parent directory.
PAYLOAD_LONGEST_RELATIVE = 132
# Conservative ceiling for a staged path. The shipped payload needs
# 179 characters after the base (separators, workspace name, media/payload
# and the longest member), which leaves 41 characters for the base itself and
# keeps ~39 characters of headroom below MAX_PATH for indexers, antivirus
# handles and temporary files.
STAGED_PATH_BUDGET = 220
WORKSPACE_RANDOM = 8
WORKSPACE_FINALIZE_PREFIX = ".worker-media-finalize-"
WORKSPACE_TEMPLATE_PREFIX = ".worker-media-template-"
WORKSPACE_CHECK_PREFIX = ".worker-media-check-"
MEDIA_PAYLOAD_RELATIVE = os.path.join("media", "payload")
WORKSPACE_MARGIN = (
    len(os.sep)
    + len(WORKSPACE_FINALIZE_PREFIX)
    + WORKSPACE_RANDOM
    + len(os.sep + MEDIA_PAYLOAD_RELATIVE)
    + len(os.sep)
    + PAYLOAD_LONGEST_RELATIVE
)


def _hash(value):
    return isinstance(value, str) and HASH.fullmatch(value) is not None


def _load_template(directory):
    template = no_links(directory)
    if not template.is_dir():
        raise ValueError("Offline media template directory is missing")
    record = json.loads(regular(template / "media-template.json").read_text(encoding="utf-8"))
    if not isinstance(record, dict):
        raise ValueError("Unsupported offline media template record")
    binaries = record.get("binaries")
    if (
        record.get("schema") != TEMPLATE_SCHEMA
        or not isinstance(record.get("version"), str)
        or VERSION.fullmatch(record["version"]) is None
        or not isinstance(record.get("source_revision"), str)
        or re.fullmatch(r"[0-9a-f]{40}", record["source_revision"]) is None
        or not _hash(record.get("payload_manifest_draft_sha256"))
        or not _hash(record.get("toolchain_lock_sha256"))
        or not isinstance(binaries, dict)
        or set(binaries) != set(BINARIES)
        or any(
            not isinstance(item, dict)
            or not _hash(item.get("sha256"))
            or type(item.get("size")) is not int
            or item["size"] <= 0
            for item in binaries.values()
        )
    ):
        raise ValueError("Unsupported offline media template record")
    draft = regular(template / "manifest-draft.json")
    if sha256(draft) != record["payload_manifest_draft_sha256"]:
        raise ValueError("Offline media template manifest draft changed")
    manifest = json.loads(draft.read_text(encoding="utf-8"))
    files = manifest.get("files") if isinstance(manifest, dict) else None
    worker = files.get("runtime/native/wukong-worker.exe") if isinstance(files, dict) else None
    if (
        not isinstance(manifest, dict)
        or manifest.get("schema") != 2
        or manifest.get("platform") != "windows-amd64"
        or manifest.get("version") != record["version"]
        or manifest.get("source_revision") != record["source_revision"]
        or manifest.get("toolchain_lock_sha256") != record["toolchain_lock_sha256"]
        or not isinstance(worker, dict)
        or worker.get("sha256") != binaries["wukong-worker.exe"]["sha256"]
        or worker.get("size") != binaries["wukong-worker.exe"]["size"]
    ):
        raise ValueError("Offline media template manifest draft differs from its record")
    native = json.loads(regular(template / "native-build.json").read_text(encoding="utf-8"))
    payload = template / "payload"
    if not payload.is_dir():
        raise ValueError("Offline media template payload is missing")
    if sha256(regular(payload / "native-build.json")) != sha256(
        regular(template / "native-build.json")
    ):
        raise ValueError("Offline media template native build copies differ")
    if sha256(regular(payload / "toolchain.lock.json")) != record["toolchain_lock_sha256"]:
        raise ValueError("Offline media template toolchain lock changed")
    native_files = native.get("files") if isinstance(native, dict) else None
    if (
        not isinstance(native, dict)
        or native.get("schema") != TEMPLATE_SCHEMA
        or native.get("platform") != "windows-amd64"
        or native.get("source_revision") != record["source_revision"]
        or not isinstance(native_files, dict)
        or set(native_files) != set(BINARIES)
        or any(
            not isinstance(native_files[name], dict)
            or native_files[name].get("sha256") != binaries[name]["sha256"]
            or native_files[name].get("size") != binaries[name]["size"]
            for name in BINARIES
        )
    ):
        raise ValueError("Offline media template native build metadata differs")
    for name in BINARIES:
        path = regular(template / "binaries" / name)
        if (
            path.stat().st_size != binaries[name]["size"]
            or sha256(path) != binaries[name]["sha256"]
        ):
            raise ValueError("Offline media template binary changed: " + name)
    return dict(
        path=template,
        record=record,
        manifest=manifest,
        binaries=template / "binaries",
        payload=payload,
    )


def _publish_directory(source, target):
    # Windows antivirus/indexer handles can briefly block renaming a freshly
    # written media tree. Retry a bounded number of times, but never overwrite.
    # A short TEMP staging workspace may live on another volume than the output,
    # where os.replace fails with EXDEV; copy the finished tree instead.
    for attempt in range(10):
        try:
            os.replace(source, target)
            return
        except OSError as error:
            last = attempt == 9
            if target.exists():
                raise ValueError(
                    "Offline media output could not be published: " + str(error)
                ) from error
            if error.errno != errno.EXDEV:
                if last:
                    raise ValueError(
                        "Offline media output could not be published: " + str(error)
                    ) from error
                time.sleep(0.5 * (attempt + 1))
                continue
            try:
                _copy_tree(source, target)
            except (OSError, ValueError) as copy_error:
                shutil.rmtree(target, ignore_errors=True)
                if last:
                    raise ValueError(
                        "Offline media output could not be published: " + str(copy_error)
                    ) from copy_error
                time.sleep(0.5 * (attempt + 1))
                continue
            shutil.rmtree(source, ignore_errors=True)
            return


def _workspace_candidates(output_parent):
    candidates = []
    for variable in ("TEMP", "TMP"):
        value = os.environ.get(variable)
        if value:
            candidates.append((variable, Path(value)))
    candidates.append(("system temporary directory", Path(tempfile.gettempdir())))
    if output_parent is not None:
        candidates.append(("output parent", Path(output_parent)))
    return candidates


def _workspace_base(output_parent):
    """Pick an absolute existing base whose staged payload path fits Windows.

    TEMP/TMP wins over the system temporary directory, and both win over the
    output parent, which can sit under a deep operator state directory. Every
    candidate must leave WORKSPACE_MARGIN characters for the workspace name,
    the media/payload copies and the deepest payload member.
    """
    rejected = []
    for label, candidate in _workspace_candidates(output_parent):
        if not candidate.is_absolute():
            rejected.append(f"{label} {candidate} is not absolute")
            continue
        candidate = Path(os.path.abspath(candidate))
        required = len(str(candidate)) + WORKSPACE_MARGIN
        if required > STAGED_PATH_BUDGET:
            rejected.append(f"{label} {candidate} needs {required} characters")
            continue
        if not candidate.is_dir():
            rejected.append(f"{label} {candidate} is not an existing directory")
            continue
        return candidate
    raise ValueError(
        "Offline media staging needs a shorter absolute workspace directory: "
        + "; ".join(rejected)
        + f". A workspace base may be at most {STAGED_PATH_BUDGET - WORKSPACE_MARGIN} "
        "characters and must already exist; move the controller state_dir (and finalized "
        "media output) to a shorter absolute path, for example C:\\state or D:\\state, or "
        "point TEMP/TMP at a short absolute directory."
    )


def _workspace(output_parent, prefix):
    return Path(tempfile.mkdtemp(dir=_workspace_base(output_parent), prefix=prefix))


def _discard_workspace(workspace):
    """Remove a staging workspace, retrying transient Windows file locks."""
    for attempt in range(5):
        shutil.rmtree(workspace, ignore_errors=True)
        if not workspace.exists():
            return
        time.sleep(0.5 * (attempt + 1))


def _longest_member(manifest):
    files = manifest.get("files") if isinstance(manifest, dict) else None
    if not isinstance(files, dict) or not files:
        raise ValueError("Offline media template manifest lists no payload members")
    return max(files, key=len)


def _check_output_length(output, longest, member=None):
    """Reject a too-deep final media output before anything is copied there."""
    required = len(str(output)) + len(os.sep) + len("payload") + len(os.sep) + longest
    if required <= WINDOWS_PATH_LIMIT:
        return
    offending = Path(output) / "payload"
    if member is not None:
        offending /= member
    room = WINDOWS_PATH_LIMIT - (len(os.sep) + len("payload") + len(os.sep) + longest)
    raise ValueError(
        "Offline media output path is too long for Windows MAX_PATH: "
        f"{required} characters are needed for the deepest payload member "
        f"(limit {WINDOWS_PATH_LIMIT}); the destination {offending} cannot hold the "
        f"payload. The output directory may be at most {room} characters; move the "
        "controller state_dir (or finalized media output) to a shorter absolute path."
    )


def _copy_tree(source, destination):
    source = no_links(source)
    if not source.is_dir():
        raise ValueError("Offline media template payload is missing")
    for current, directories, files in os.walk(source, followlinks=False):
        current = Path(current)
        target = no_links(destination / current.relative_to(source))
        for name in sorted(directories):
            no_links(current / name)  # linked directories never enter controller staging
        target.mkdir(parents=True, exist_ok=True)
        for name in sorted(files):
            shutil.copyfile(regular(current / name), target / name)


def _ssh_public_key(value):
    if not isinstance(value, dict) or set(value) != {"algorithm", "key"}:
        raise ValueError("Invalid offline media SSH public key")
    algorithm, key = value["algorithm"], value["key"]
    if algorithm != "ssh-ed25519" or not isinstance(key, str) or len(key) != 68:
        raise ValueError("Only a canonical Ed25519 SSH public key is supported")
    try:
        blob = base64.b64decode(key, validate=True)
    except (TypeError, ValueError) as error:
        raise ValueError("Invalid offline media SSH public key") from error
    if (
        len(blob) != 51
        or blob[: len(SSH_ED25519_PREFIX)] != SSH_ED25519_PREFIX
        or base64.b64encode(blob).decode("ascii") != key
    ):
        raise ValueError("Invalid offline media SSH public key")
    return dict(algorithm="ssh-ed25519", key=key)


def read_spec(path):
    raw = regular(path).read_bytes()
    if len(raw) > 64 * 1024:
        raise ValueError("Offline media enrollment spec exceeds 64 KiB")
    value = json.loads(raw)
    if not isinstance(value, dict) or set(value) - SPEC_FIELDS or value.get("schema") != 1:
        raise ValueError("Unsupported offline media enrollment spec")
    installation = value.get("installation_id")
    if not isinstance(installation, str) or re.fullmatch(r"[0-9a-f]{32}", installation) is None:
        raise ValueError("Invalid offline media installation identity")
    environment = value.get("environment_id")
    if (
        not isinstance(environment, str)
        or re.fullmatch(r"[A-Za-z0-9_-]{1,64}", environment) is None
    ):
        raise ValueError("Invalid offline media environment identity")
    generation = value.get("generation", 1)
    if type(generation) is not int or generation < 1:
        raise ValueError("Invalid offline media enrollment generation")
    address = value.get("management_peer")
    if not isinstance(address, str):
        raise ValueError("Invalid offline media management address")
    try:
        management = ipaddress.ip_address(address)
    except ValueError as error:
        raise ValueError("Invalid offline media management address") from error
    if (
        management.version != 4
        or management.is_unspecified
        or management.is_loopback
        or management.is_multicast
        or int(management) == 0xFFFFFFFF
        or str(management) != address
    ):
        raise ValueError("Invalid offline media management address")
    port = value.get("ssh_port")
    if type(port) is not int or not 1 <= port <= 65535:
        raise ValueError("Invalid offline media SSH port")
    if value.get("execution_boundary", "disposable_vm") != "disposable_vm":
        raise ValueError("Offline media requires the disposable_vm execution boundary")
    ssh = value.get("ssh_public_key")
    return dict(
        installation_id=installation,
        environment_id=environment,
        generation=generation,
        management_peer=address,
        ssh_port=port,
        execution_boundary="disposable_vm",
        ssh_public_key=_ssh_public_key(ssh) if ssh is not None else None,
    )


def _run(command):
    return subprocess.run(command, check=True, capture_output=True, text=True).stdout


def _run_json(command):
    return json.loads(_run(command))


def _outside(path, roots, label):
    for root in roots:
        if path == root or root in path.parents or path in root.parents:
            raise ValueError(label + " must stay outside the offline media template and output")


def _previous(previous_template, previous_enrollment):
    if (previous_template is None) != (previous_enrollment is None):
        raise ValueError("Recovery media requires both previous public sources")
    if previous_template is None:
        return None, None
    previous_template = no_links(previous_template)
    previous_enrollment = no_links(previous_enrollment)
    package = previous_template / "payload"
    if not package.is_dir() or not previous_enrollment.is_dir():
        raise ValueError("Previous finalized media payload or enrollment is missing")
    if {path.name for path in previous_enrollment.iterdir()} != {
        "enrollment.json",
        "enrollment.sig",
    }:
        raise ValueError("Previous enrollment must contain only the two public signed records")
    regular(package / "manifest.json")
    regular(package / "manifest.sig")
    return package, previous_enrollment


def _generate_ssh_key(keygen, private):
    private = no_links(private)
    subprocess.run(
        [str(regular(keygen)), "-q", "-t", "ed25519", "-N", "", "-f", str(private)],
        check=True,
        capture_output=True,
        text=True,
    )
    fields = regular(Path(str(private) + ".pub")).read_text(encoding="utf-8").split()
    if len(fields) < 2:
        raise ValueError("Generated SSH public key is incomplete")
    return _ssh_public_key(dict(algorithm=fields[0], key=fields[1]))


def _private_names(keys, ssh_private):
    names = {"signing-key.bin"}
    keys = no_links(keys)
    if keys.is_dir():
        names.update(path.name for path in keys.iterdir())
    if ssh_private is not None:
        names.add(no_links(ssh_private).name)
    return names


def _assert_private_absent(root, private):
    for path in sorted(root.rglob("*")):
        no_links(path)
        if path.is_file() and path.name in private:
            raise ValueError("Private key material must stay outside offline media: " + path.name)


def _verify_media(media, record, template, *, expect_recovery, private):
    media = no_links(media)
    entries = {path.name for path in media.iterdir()}
    if ALLOWLIST - entries or entries - (ALLOWLIST | {RECOVERY}):
        raise ValueError("Offline media layout differs from the supported set")
    if (RECOVERY in entries) != expect_recovery:
        raise ValueError("Offline media recovery files differ from the requested sources")
    if json.loads(regular(media / "media.json").read_text(encoding="utf-8")) != record:
        raise ValueError("Offline media receipt differs from the finalized media")
    setup = json.loads(regular(media / "setup.json").read_text(encoding="utf-8"))
    if (
        not isinstance(setup, dict)
        or setup.get("schema") != 1
        or setup.get("controller_public_key") != record["controller_public_key"]
        or setup.get("controller_version") != record["version"]
    ):
        raise ValueError("Offline media setup record differs from the receipt")
    worker = sha256(regular(template["binaries"] / "wukong-worker.exe"))
    if record["setup_sha256"] != worker:
        raise ValueError("Offline media setup executable differs from the staged worker")
    if sha256(regular(media / "WukongWorkerSetup.exe")) != record["setup_sha256"]:
        raise ValueError("Offline media setup executable changed")
    enrollment = no_links(media / "enrollment")
    if {path.name for path in enrollment.iterdir()} != {"enrollment.json", "enrollment.sig"}:
        raise ValueError("Offline media enrollment records are incomplete")
    if sha256(regular(enrollment / "enrollment.json")) != record["enrollment_sha256"]:
        raise ValueError("Offline media enrollment record changed")
    if sha256(regular(media / "payload/manifest.json")) != record["manifest_sha256"]:
        raise ValueError("Offline media payload manifest changed")
    regular(media / "payload/manifest.sig")
    if RECOVERY in entries:
        recovery = no_links(media / RECOVERY)
        if {path.name for path in recovery.iterdir()} != {
            "manifest.json",
            "manifest.sig",
            "enrollment.json",
            "enrollment.sig",
        }:
            raise ValueError("Offline media recovery records are incomplete")
    _assert_private_absent(media, private)


def _write_report(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    handle = tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", dir=path.parent, prefix=".worker-media-report-", delete=False
    )
    temporary = Path(handle.name)
    try:
        with handle:
            handle.write(json.dumps(value, indent=2) + "\n")
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def template(cache, native, output, requested_version=None):
    if platform.system() != "Windows" or platform.machine().lower() not in {"amd64", "x86_64"}:
        raise ValueError("Offline media templates require Windows x64")
    output = no_links(output)
    if output.exists():
        raise ValueError("Offline media template output already exists")
    _check_output_length(output, PAYLOAD_LONGEST_RELATIVE)
    expected = version()
    if requested_version is not None and requested_version != expected:
        raise ValueError("Requested media version differs from the repository version")
    output.parent.mkdir(parents=True, exist_ok=True)
    workspace = _workspace(output.parent, WORKSPACE_TEMPLATE_PREFIX)
    try:
        staged = stage(cache, workspace / "payload", native)
        payload = workspace / "payload"
        binaries = workspace / "binaries"
        binaries.mkdir()
        records = {}
        for name in BINARIES:
            shutil.copyfile(regular(staged["binaries"] / name), binaries / name)
            records[name] = dict(
                sha256=sha256(binaries / name), size=(binaries / name).stat().st_size
            )
        shutil.copyfile(regular(payload / "native-build.json"), workspace / "native-build.json")
        draft = workspace / "manifest-draft.json"
        draft.write_text(json.dumps(staged["manifest"]), encoding="utf-8")
        record = dict(
            schema=TEMPLATE_SCHEMA,
            version=expected,
            source_revision=staged["native_build"]["source_revision"],
            payload_manifest_draft_sha256=sha256(draft),
            binaries=records,
            toolchain_lock_sha256=sha256(regular(payload / "toolchain.lock.json")),
        )
        (workspace / "media-template.json").write_text(
            json.dumps(record, indent=2) + "\n", encoding="utf-8"
        )
        _publish_directory(workspace, output)
    finally:
        _discard_workspace(workspace)
    return dict(
        prepared=True,
        scope="unsigned_offline_media_template",
        output=str(output),
        version=record["version"],
        source_revision=record["source_revision"],
        payload_manifest_draft_sha256=record["payload_manifest_draft_sha256"],
        toolchain_lock_sha256=record["toolchain_lock_sha256"],
    )


def finalize(
    template_dir,
    keys,
    spec,
    output,
    controller_version=None,
    generate_ssh_key=None,
    previous_template=None,
    previous_enrollment=None,
):
    template = _load_template(template_dir)
    keys = Path(keys)
    if not keys.is_absolute():
        raise ValueError("Controller key storage must be an absolute directory")
    keys = no_links(keys)
    output = no_links(output)
    if output.exists():
        raise ValueError("Offline media output already exists")
    longest = _longest_member(template["manifest"])
    _check_output_length(output, len(longest), longest)
    _outside(keys, [template["path"], output], "Controller key storage")
    if not keys.exists() and not keys.parent.is_dir():
        raise ValueError("Controller key directory parent is missing")
    requested = read_spec(spec)
    media_version = controller_version or template["record"]["version"]
    if VERSION.fullmatch(media_version) is None:
        raise ValueError("Invalid controller version")
    if tuple(map(int, media_version.split("."))) < tuple(
        map(int, template["record"]["version"].split("."))
    ):
        raise ValueError("Controller version is older than the offline media template")
    ssh_private = None
    if generate_ssh_key is not None:
        if requested["ssh_public_key"] is not None:
            raise ValueError("Provide either ssh_public_key in the spec or --generate-ssh-key")
        ssh_private = Path(generate_ssh_key)
        if not ssh_private.is_absolute():
            raise ValueError("Generated SSH private key path must be absolute")
        ssh_private = no_links(ssh_private)
        _outside(ssh_private, [template["path"], output], "Generated SSH private key")
        if ssh_private.exists() or Path(str(ssh_private) + ".pub").exists():
            raise ValueError("Generated SSH private key output already exists")
        if not ssh_private.parent.is_dir():
            raise ValueError("Generated SSH private key parent directory is missing")
    elif requested["ssh_public_key"] is None:
        raise ValueError("Enrollment spec must include ssh_public_key or --generate-ssh-key")
    previous_package, previous_enrollment_dir = _previous(previous_template, previous_enrollment)
    output.parent.mkdir(parents=True, exist_ok=True)
    workspace = _workspace(output.parent, WORKSPACE_FINALIZE_PREFIX)
    try:
        package = workspace / "payload"
        _copy_tree(template["payload"], package)
        if ssh_private is not None:
            requested["ssh_public_key"] = _generate_ssh_key(
                package / "runtime/openssh/ssh-keygen.exe", ssh_private
            )
        package_cli = template["binaries"] / "wukong-package.exe"
        public = _run_json([str(package_cli), "key", str(keys)])
        public_key = public.get("public_key") if isinstance(public, dict) else None
        if not _hash(public_key) or not _hash(public.get("key_sha256")):
            raise ValueError("Controller signing utility did not return a public key")
        sealed = _run_json(
            [
                str(package_cli),
                "seal",
                str(package),
                str(template["path"] / "manifest-draft.json"),
                str(keys),
                media_version,
            ]
        )
        manifest_sha = sealed.get("manifest_sha256") if isinstance(sealed, dict) else None
        if (
            sealed.get("sealed") is not True
            or not _hash(manifest_sha)
            or sealed.get("key_sha256") != public["key_sha256"]
        ):
            raise ValueError("Controller sealing did not bind the staged payload")
        if sha256(regular(package / "manifest.json")) != manifest_sha:
            raise ValueError("Sealed manifest differs from the reported identity")
        sealed_manifest = json.loads(regular(package / "manifest.json").read_text(encoding="utf-8"))
        if sealed_manifest.get("version") != media_version:
            raise ValueError("Sealed worker manifest version differs from the offline media version")
        regular(package / "manifest.sig")
        enrollment = dict(
            schema=1,
            generation=requested["generation"],
            installation_id=requested["installation_id"],
            environment_id=requested["environment_id"],
            controller_public_key=public_key,
            payload_manifest_sha256=manifest_sha,
            management_peer=requested["management_peer"],
            ssh_port=requested["ssh_port"],
            ssh_public_key=requested["ssh_public_key"],
            execution_boundary=requested["execution_boundary"],
        )
        draft = workspace / "enrollment-draft.json"
        draft.write_text(json.dumps(enrollment), encoding="utf-8")
        enrollment_dir = workspace / "enrollment"
        _run(
            [
                str(package_cli),
                "enroll",
                str(package),
                str(draft),
                str(keys),
                str(enrollment_dir),
                media_version,
            ]
        )
        enrollment_dir = no_links(enrollment_dir)
        if not enrollment_dir.is_dir() or {path.name for path in enrollment_dir.iterdir()} != {
            "enrollment.json",
            "enrollment.sig",
        }:
            raise ValueError("Signed enrollment records are incomplete")
        media = workspace / "media"
        receipt = prepare_bundle(
            package,
            enrollment_dir,
            media,
            public_key,
            media_version,
            template["binaries"] / "wukong-worker.exe",
            previous_package,
            previous_enrollment_dir,
        )
        record = dict(
            schema=MEDIA_SCHEMA,
            version=media_version,
            controller_public_key=public_key,
            manifest_sha256=manifest_sha,
            setup_sha256=sha256(regular(media / "WukongWorkerSetup.exe")),
            enrollment_sha256=sha256(regular(media / "enrollment/enrollment.json")),
            installation_id=requested["installation_id"],
            environment_id=requested["environment_id"],
            generation=requested["generation"],
            source_revision=template["manifest"]["source_revision"],
        )
        if (
            receipt.get("manifest_sha256") != manifest_sha
            or receipt.get("installation_id") != requested["installation_id"]
        ):
            raise ValueError("Offline setup media differs from the sealed payload")
        (media / "media.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
        _verify_media(
            media,
            record,
            template,
            expect_recovery=previous_package is not None,
            private=_private_names(keys, ssh_private),
        )
        _publish_directory(media, output)
    finally:
        _discard_workspace(workspace)
    return record


def check(template_dir, report):
    template = _load_template(template_dir)
    media_version = template["record"]["version"]
    scratch = _workspace(None, WORKSPACE_CHECK_PREFIX)
    try:
        keys = scratch / "keys"
        generated = scratch / "ssh/management-ed25519"
        (scratch / "ssh").mkdir()
        spec = scratch / "spec.json"
        spec.write_text(
            json.dumps(
                dict(
                    schema=1,
                    installation_id=uuid.uuid4().hex,
                    environment_id="offline-media-check",
                    generation=1,
                    management_peer="192.0.2.10",
                    ssh_port=2222,
                )
            ),
            encoding="utf-8",
        )
        output = scratch / "media"
        record = finalize(
            template["path"],
            keys,
            spec,
            output,
            controller_version=media_version,
            generate_ssh_key=generated,
        )
        setup = json.loads(regular(output / "setup.json").read_text(encoding="utf-8"))
        verified = _run_json(
            [
                str(regular(output / "WukongWorkerSetup.exe")),
                "verify-enrollment",
                str(no_links(output / "payload")),
                str(no_links(output / "enrollment")),
                setup.get("controller_public_key"),
                media_version,
            ]
        )
        if (
            not isinstance(verified, dict)
            or verified.get("verified") is not True
            or verified.get("execution_boundary") != "disposable_vm"
            or verified.get("manifest_sha256") != record["manifest_sha256"]
            or verified.get("installation_id") != record["installation_id"]
        ):
            raise ValueError("Offline media setup verification did not accept the signed media")
        if (
            sha256(regular(output / "WukongWorkerSetup.exe"))
            != template["record"]["binaries"]["wukong-worker.exe"]["sha256"]
        ):
            raise ValueError("Offline media setup executable differs from the template worker")
        _verify_media(
            output,
            record,
            template,
            expect_recovery=False,
            private=_private_names(keys, generated),
        )
        report_value = dict(
            passed=True,
            version=media_version,
            template=str(template["path"]),
            setup_sha256=record["setup_sha256"],
            manifest_sha256=record["manifest_sha256"],
            enrollment_sha256=record["enrollment_sha256"],
            verified=True,
        )
        _write_report(report, report_value)
        return report_value
    finally:
        _discard_workspace(scratch)


def pack(template_dir, media_version, output):
    template = _load_template(template_dir)
    if media_version != template["record"]["version"]:
        raise ValueError("Packed version differs from the offline media template version")
    output = Path(output)
    if output.suffix.lower() != ".zip":
        output = output / (ARCHIVE_ROOT + "-" + media_version + ".zip")
    output = no_links(output)
    if output.exists():
        raise ValueError("Offline media archive already exists")
    if output == template["path"] or template["path"] in output.parents:
        raise ValueError("Offline media archive must stay outside the template")
    output.parent.mkdir(parents=True, exist_ok=True)
    entries = []
    for current, directories, files in os.walk(template["path"], followlinks=False):
        current = Path(current)
        for name in directories:
            no_links(current / name)
        for name in files:
            path = regular(current / name)
            entries.append((path.relative_to(template["path"]).as_posix(), path))
    entries.sort()
    descriptor, temporary = tempfile.mkstemp(
        dir=output.parent, prefix=".worker-media-archive-", suffix=".zip"
    )
    os.close(descriptor)
    temporary = Path(temporary)
    try:
        with zipfile.ZipFile(temporary, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for relative, path in entries:
                info = zipfile.ZipInfo(
                    ARCHIVE_ROOT + "-" + media_version + "/" + member(relative),
                    date_time=(1980, 1, 1, 0, 0, 0),
                )
                info.compress_type = zipfile.ZIP_DEFLATED
                info.create_system = 3
                info.external_attr = 0o644 << 16
                with regular(path).open("rb") as incoming, archive.open(info, "w") as outgoing:
                    shutil.copyfileobj(incoming, outgoing, 1024 * 1024)
        os.replace(temporary, output)
    finally:
        temporary.unlink(missing_ok=True)
    return dict(
        packed=True,
        archive=str(output),
        sha256=sha256(output),
        size=output.stat().st_size,
    )


def iso(media, mkisofs, output):
    media = no_links(media)
    if not media.is_dir():
        raise ValueError("Finalized offline media directory is missing")
    mkisofs = Path(mkisofs)
    if not mkisofs.is_absolute():
        raise ValueError("mkisofs executable must be an absolute path")
    mkisofs = regular(mkisofs)
    output = no_links(output)
    if output.exists():
        raise ValueError("Offline ISO output already exists")
    output.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            str(mkisofs),
            "-quiet",
            "-iso-level",
            "4",
            "-joliet",
            "-joliet-long",
            "-V",
            "WUKONG_OFFLINE",
            "-o",
            str(output),
            str(media),
        ],
        check=True,
    )
    return dict(
        created=True,
        scope="offline_media_iso",
        output=str(output),
        label="WUKONG_OFFLINE",
        size=output.stat().st_size,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prepare = commands.add_parser(
        "template", help="Stage the unsigned payload, manifest draft and binaries in CI"
    )
    prepare.add_argument("--cache", type=Path, required=True)
    prepare.add_argument("--native", type=Path, default=ROOT / "target/windows-worker-native")
    prepare.add_argument("--output", type=Path, required=True)
    prepare.add_argument("--version")
    seal = commands.add_parser(
        "finalize",
        help="Seal the payload, sign enrollment and bundle setup media with the controller key directory",
    )
    seal.add_argument("--template", type=Path, required=True)
    seal.add_argument("--keys", type=Path, required=True)
    seal.add_argument("--spec", type=Path, required=True)
    seal.add_argument("--output", type=Path, required=True)
    seal.add_argument("--controller-version")
    seal.add_argument("--generate-ssh-key", type=Path)
    seal.add_argument("--previous-template", type=Path)
    seal.add_argument("--previous-enrollment", type=Path)
    verify = commands.add_parser(
        "check", help="Finalize and verify a template with an ephemeral key (no elevation)"
    )
    verify.add_argument("--template", type=Path, required=True)
    verify.add_argument("--report", type=Path, required=True)
    archive = commands.add_parser(
        "pack", help="Write the deterministic offline media release archive"
    )
    archive.add_argument("--template", type=Path, required=True)
    archive.add_argument("--version", required=True)
    archive.add_argument("--output", type=Path, required=True)
    image = commands.add_parser("iso", help="Wrap finalized offline media in an ISO image")
    image.add_argument("--media", type=Path, required=True)
    image.add_argument("--mkisofs", type=Path, required=True)
    image.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "template":
            result = template(args.cache, args.native, args.output, args.version)
        elif args.command == "finalize":
            result = finalize(
                args.template,
                args.keys,
                args.spec,
                args.output,
                args.controller_version,
                args.generate_ssh_key,
                args.previous_template,
                args.previous_enrollment,
            )
        elif args.command == "check":
            result = check(args.template, args.report)
        elif args.command == "pack":
            result = pack(args.template, args.version, args.output)
        else:
            result = iso(args.media, args.mkisofs, args.output)
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Offline worker media operation failed: {error}\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
