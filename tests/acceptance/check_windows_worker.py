#!/usr/bin/env python3
"""Install trusted offline media only on an explicitly selected hosted test VM.

Controller acquisition is online. Installation blocks external network access
for shipped executables, while the CI agent stays connected. This is not the
final disconnected Windows 10 VM or production target-isolation acceptance.
"""
import argparse
import base64
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import platform
import socket
import subprocess
import sys
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))


def require_hosted(confirmed):
    if (not confirmed or platform.system() != 'Windows'
            or os.environ.get('GITHUB_ACTIONS') != 'true'
            or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted'):
        raise RuntimeError('Installation fixture requires an explicitly confirmed dedicated GitHub-hosted Windows runner')
    temporary = Path(os.environ['RUNNER_TEMP']).resolve()
    if not temporary.is_absolute() or not temporary.is_dir():
        raise RuntimeError('Hosted runner temporary directory is unavailable')
    return temporary


def run(arguments, timeout=300, env=None):
    result = subprocess.run(list(map(str, arguments)), cwd=ROOT, env=env, stdin=subprocess.DEVNULL,
                            capture_output=True, text=True, encoding='utf-8', errors='replace', timeout=timeout)
    if result.returncode:
        # Arguments can contain local key paths. Never dump the command, SSH
        # frames, signing material or worker metadata into the public report.
        raise RuntimeError(f'{Path(arguments[0]).name} failed ({result.returncode}): {result.stderr[-2048:]}')
    if len(result.stdout) > 4 * 1024 * 1024:
        raise RuntimeError('Fixture command output exceeds its bound')
    return result.stdout.strip()


def save(path, value):
    with path.open('x', encoding='utf-8') as output:
        json.dump(value, output, indent=2)
        output.write('\n')


def local_management_ip():
    for info in socket.getaddrinfo(socket.gethostname(), 0, socket.AF_INET, socket.SOCK_STREAM):
        address = ipaddress.IPv4Address(info[4][0])
        if address.is_loopback or address.is_unspecified or address.is_multicast:
            continue
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as proof:
            proof.bind((str(address), 0))
        return str(address)
    raise RuntimeError('Hosted fixture has no assigned non-loopback IPv4 management address')


def external_addresses(management):
    address = ipaddress.IPv4Address(management)
    if address.is_loopback or address.is_unspecified or address.is_multicast:
        raise ValueError('Fixture management must match the enrollment address contract')
    exclusions = sorted([(int(ipaddress.IPv4Address('127.0.0.0')), int(ipaddress.IPv4Address('127.255.255.255'))),
                         (int(address), int(address))])
    result, start = [], 0
    for low, high in exclusions:
        if start < low:
            result.append(f'{ipaddress.IPv4Address(start)}-{ipaddress.IPv4Address(low-1)}')
        start = high + 1
    if start <= 0xffffffff:
        result.append(f'{ipaddress.IPv4Address(start)}-255.255.255.255')
    result.append('::2-ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff')
    return result


def diagnostics(installed):
    """Only fixed native diagnostics; never endpoint/lease/LSA/key records."""
    metadata = installed / 'metadata'
    selected = [metadata / 'last-service-error.txt', metadata / 'sshd-native.log']
    selected += sorted(metadata.glob('selftest-*.log'))[-2:]
    result = {}
    for path in selected:
        if path.is_file() and not path.is_symlink():
            try:
                with path.open('rb') as incoming:
                    incoming.seek(max(0, path.stat().st_size - 4096))
                    result[path.name] = incoming.read(4096).decode('utf-8', errors='replace')
            except OSError:
                result[path.name] = 'Diagnostic could not be read while the service held it.'
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--confirm-hosted-runner', action='store_true')
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    runner_temp = require_hosted(args.confirm_hosted_runner)  # Before any mutation.
    from scripts.worker.assemble import assemble
    from scripts.worker.bundle import prepare
    from scripts.worker.toolchain import acquire, load_lock

    work = Path(tempfile.mkdtemp(prefix='wukong-hosted-worker-', dir=runner_temp)).resolve()
    report_path = args.report.absolute()
    report_path.parent.mkdir(parents=True, exist_ok=True)
    if report_path.exists():
        raise RuntimeError('Fixture report output already exists')
    installation = uuid.uuid4().hex
    helper = ROOT / 'tests/acceptance/windows-worker-hosted.ps1'
    plan_path = work / 'hosted-plan.json'
    installed = Path(os.environ['ProgramData']) / 'WukongWorkers' / installation
    report = dict(passed=False, scope='hosted_native_bootstrap_service_and_controller_ssh',
                  installation_id=installation, whole_vm_offline_verified=False,
                  production_isolation_verified=False, production_uninstall_verified=False)

    def control(mode):
        output = run(['pwsh', '-NoProfile', '-File', helper, '-Mode', mode,
                      '-Plan', plan_path, '-ConfirmHostedRunner'], timeout=300)
        return json.loads(output)

    try:
        cache, package, keys, enrollment, media = [work / name for name in ['cache', 'payload', 'controller-private', 'enrollment', 'media']]
        native = ROOT / 'target/windows-worker-native/x86_64-pc-windows-msvc/release'
        cli = native / 'wukong-package.exe'
        public = json.loads(run([cli, 'key', keys]))
        report['controller_signing_fingerprint'] = public['key_sha256']
        # No signing/SSH private key enters the media or uploaded evidence.
        assets = acquire(cache, load_lock(), download=True)
        sealed = assemble(cache, package, ROOT / 'target/windows-worker-native', keys)
        manifest = json.loads((package / 'manifest.json').read_text(encoding='utf-8'))
        report.update(manifest_sha256=sealed['manifest_sha256'], source_revision=manifest['source_revision'])
        ssh_private = keys / 'management-ed25519'
        run([package / 'runtime/openssh/ssh-keygen.exe', '-q', '-t', 'ed25519', '-N', '', '-f', ssh_private])
        fields = ssh_private.with_suffix('.pub').read_text(encoding='utf-8').split()
        with socket.socket() as candidate:
            candidate.bind(('0.0.0.0', 0))
            port = candidate.getsockname()[1]
        management = local_management_ip()
        draft = dict(schema=1, generation=1, installation_id=installation, environment_id='hosted-windows',
                     controller_public_key=public['public_key'], payload_manifest_sha256=sealed['manifest_sha256'],
                     management_peer=management, ssh_port=port,
                     ssh_public_key=dict(algorithm=fields[0], key=fields[1]))
        draft_path = work / 'enrollment-draft.json'
        save(draft_path, draft)
        run([cli, 'enroll', package, draft_path, keys, enrollment, manifest['version']])
        prepare(package, enrollment, media, public['public_key'], manifest['version'], native / 'wukong-worker.exe')
        executables = [name for name in manifest['files'] if name.lower().endswith('.exe')]
        programs = [media / 'WukongWorkerSetup.exe', installed / 'bootstrap/wukong-worker.exe']
        for name in executables:
            programs.extend([media / 'payload' / name, installed / 'versions' / sealed['manifest_sha256'] / name])
        save(plan_path, dict(installation_id=installation, manifest_sha256=sealed['manifest_sha256'],
                            media=str(media), installed_root=str(installed), programs=sorted(set(map(str, programs))),
                            management_ip=management, external_addresses=external_addresses(management)))
        report['network'] = control('Block')
        reviewed = hashlib.sha256((enrollment / 'enrollment.json').read_bytes()).hexdigest()
        installed_result = json.loads(run([media / 'WukongWorkerSetup.exe', 'install-reviewed', media / 'setup.json',
                                          public['public_key'], reviewed], timeout=360))
        if (installed_result.get('installed') is not True or installed_result['installation_id'] != installation
                or installed_result['manifest_sha256'] != sealed['manifest_sha256']):
            raise RuntimeError('Installer receipt differs from prepared fixture')
        connection = json.loads((installed / 'connection.json').read_text(encoding='utf-8'))
        host = connection['host_key']
        fingerprint = 'SHA256:' + base64.b64encode(hashlib.sha256(base64.b64decode(host['key'], validate=True)).digest()).decode().rstrip('=')
        if fingerprint != connection['host_fingerprint']:
            raise RuntimeError('Installed public host fingerprint differs from its key')
        host_pin = keys / 'host-pin'
        host_pin.write_text(f'wukong-worker-{installation} {host["algorithm"]} {host["key"]}\n', encoding='utf-8', newline='\n')
        config = dict(backend='ssh', ssh_host=management, ssh_executable=str(package / 'runtime/openssh/ssh.exe'),
                      request_timeout_secs=30, managed_ssh=dict(installation_id=installation,
                      user=connection['ssh_user'], port=port, identity_file=str(ssh_private),
                      known_hosts_file=str(host_pin), host_key=host))
        reports = []
        minimum = 0
        for attempt in range(2):
            if attempt:
                report['restart'] = control('Restart')
            output = work / f'controller-{attempt}.json'
            profile = work / f'controller-profile-{attempt}.json'
            save(profile, dict(config=config, output=str(output), manifest_sha256=sealed['manifest_sha256'], minimum_epoch=minimum))
            environment = os.environ.copy()
            environment['WUKONG_INSTALLED_WORKER_TEST'] = str(profile)
            run(['cargo', 'test', '--locked', '--lib', 'installed_windows_worker_controller_acceptance',
                 '--', '--ignored', '--nocapture'], timeout=600, env=environment)
            observation = json.loads(output.read_text(encoding='utf-8'))
            if observation.get('passed') is not True:
                raise RuntimeError('Real controller acceptance did not pass')
            reports.append(observation)
            minimum = observation['last_epoch']
        if reports[0]['daemon_id'] == reports[1]['daemon_id'] or reports[0]['identity'] != reports[1]['identity']:
            raise RuntimeError('Native service restart did not change daemon epoch while retaining installation/boot/package identity')
        report.update(passed=True, controllers=reports, host_fingerprint=fingerprint,
                      offline_selftests_passed=True, pinned_assets=assets['assets'])
    except Exception as error:
        report.update(passed=False, error=f'{type(error).__name__}: {error}'[:4096])
    finally:
        if plan_path.exists() and (work / 'hosted-preflight.json').exists():
            if not report['passed']:
                report['diagnostics'] = diagnostics(installed)
            try:
                report['cleanup'] = control('Cleanup')
                if report['cleanup']['remaining_task_root']:
                    report.update(passed=False, cleanup_error='Owned installation root could not be safely removed.')
            except Exception as error:
                report.update(passed=False, cleanup_error=f'{type(error).__name__}: {error}'[:2048])
        save(report_path, report)
    print(json.dumps(report))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
