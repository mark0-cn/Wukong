"""Exercise native uninstall or real Docker cleanup only on disposable GitHub runners."""
import argparse
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import uuid


def runner_only():
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted':
        raise RuntimeError('Destructive acceptance requires a disposable GitHub-hosted runner')


def native(installer):
    runner_only()
    with tempfile.TemporaryDirectory(prefix='wukong-uninstall-') as directory:
        root = Path(directory)
        destination = root / 'program'
        env = dict(os.environ, LOCALAPPDATA=str(root / 'local'), APPDATA=str(root / 'roaming'))
        home = root / 'local/WukongData'
        home.mkdir(parents=True)
        custom = root / 'custom-cache'
        (custom / 'releases').mkdir(parents=True)
        (custom / '.wukong-owned.json').write_text(json.dumps({'schema_version': 1, 'application': 'io.wukong.desktop'}))
        (custom / 'releases/installer').write_text('owned')
        (custom / 'personal.txt').write_text('keep')
        (home / 'managed-locations.json').write_text(json.dumps({'schema_version': 1, 'application': 'io.wukong.desktop', 'locations': [{'kind': 'cache', 'path': str(custom)}]}))
        owned = home / '.wukong/artifacts/owned'
        owned.parent.mkdir(parents=True)
        owned.write_text('task evidence')
        (home / 'wukong.toml').write_text('[model]\nmodel="invalid-model-does-not-block-uninstall"\n')
        unknown = owned.parent.parent / 'personal.txt'
        unknown.write_text('keep')
        original = root / 'original.elf'
        original.write_text('original')
        browser = root / 'local/io.wukong.desktop/EBWebView/cache'
        browser.parent.mkdir(parents=True)
        browser.write_text('browser')

        def install():
            subprocess.run([str(installer), '/S', '/D=' + str(destination)], env=env, check=True, timeout=240)
            if not (destination / 'wukong.exe').is_file():
                raise AssertionError('native installation failed')

        def uninstall(*flags):
            subprocess.run([str(destination / 'uninstall.exe'), '/S', *flags, '_?=' + str(destination)], env=env, check=True, timeout=240)
            if (destination / 'wukong.exe').exists():
                raise AssertionError('native uninstaller retained executable unexpectedly')

        install()
        uninstall()
        assert owned.is_file() and browser.is_file() and original.is_file()
        install()
        uninstall('/UPDATE', '/WUKONG_DELETE_DATA')
        assert owned.is_file() and browser.is_file()
        install()
        uninstall('/WUKONG_DELETE_DATA')
        assert not owned.exists() and not browser.exists() and not (home / 'wukong.toml').exists()
        assert not (custom / 'releases/installer').exists()
        assert unknown.is_file() and original.is_file() and (custom / 'personal.txt').is_file()
        assert not (home / 'updates/cleanup-in-progress.json').exists()
    print('Real NSIS default retention, upgrade retention and explicit data cleanup passed.')


def docker(binary):
    runner_only()
    workspace = uuid.uuid4().hex[:12]
    prefix = 'wukong-' + workspace
    names = [prefix + '-tools', prefix + '-sample-service', prefix + '-foreign']
    volumes = [prefix + '-state', prefix + '-sample-state']
    networks = [prefix + '-offline', prefix + '-sample-net']
    image = prefix + ':fixture'

    def command(*args, **kwargs):
        return subprocess.check_output(['docker', *args], timeout=60, **kwargs).decode().strip()

    def exists(kind, name):
        return subprocess.run(['docker', kind, 'inspect', name], capture_output=True, timeout=30).returncode == 0

    with tempfile.TemporaryDirectory(prefix='wukong-cleanup-docker-') as directory:
        root = Path(directory)
        home = root / 'Wukong'
        bench = home / '.wukong/workbench'
        bench.mkdir(parents=True)
        (bench / 'workspace.json').write_text(json.dumps({'schema_version': 1, 'id': workspace}))
        (bench / 'challenges/sample').mkdir(parents=True)
        (bench / 'challenges/sample/challenge.toml').write_text('schema_version=1\nid="sample"\n')
        env = dict(os.environ, XDG_DATA_HOME=str(root))
        env.pop('DOCKER_CONTEXT', None)
        env.pop('DOCKER_HOST', None)
        executable = root / 'pause'
        subprocess.run(['gcc', '-static', '-x', 'c', '-o', str(executable), '-'], input=b'#include <unistd.h>\nint main(void){for(;;)sleep(1);}\n', check=True, timeout=60)
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode='w') as tar:
            tar.add(executable, arcname='pause')
        try:
            command('image', 'import', '--change', 'LABEL wukong.workspace=' + workspace, '-', image, input=archive.getvalue())
            for name in networks:
                command('network', 'create', '--internal', '--label', 'wukong.workspace=' + workspace, name)
            for name in volumes:
                command('volume', 'create', '--label', 'wukong.workspace=' + workspace, name)
            for i, name in enumerate(names[:2]):
                command('run', '-d', '--name', name, '--label', 'wukong.workspace=' + workspace,
                        '--network', networks[i], '-v', volumes[i] + ':/data', image, '/pause')
            command('run', '-d', '--name', names[2], '-v', volumes[1] + ':/data', image, '/pause')

            def clean(*flags):
                plan = root / 'review.json'
                subprocess.run([str(binary), 'cleanup', 'plan', *flags, '--output', str(plan)], env=env, check=True, timeout=120)
                value = json.loads(plan.read_text())
                assert not value['errors'], value['errors']
                subprocess.run([str(binary), 'cleanup', 'execute', '--review', str(plan), '--yes'], env=env, check=True, timeout=180)
                return value

            clean('--tools')
            assert not exists('container', names[0]) and not exists('volume', volumes[0])
            assert exists('container', names[1]) and exists('image', image)
            plan = clean('--challenges')
            assert not exists('container', names[1]) and exists('container', names[2])
            assert exists('volume', volumes[1]) and exists('image', image)
            assert any(item['reason'] == 'referenced by a retained container' for item in plan['retained'])
            command('container', 'rm', '-f', names[2])
            clean('--tools', '--challenges', '--data')
            assert not exists('volume', volumes[1]) and not exists('image', image)
        finally:
            # Explicitly named fixtures only. Never prune the runner daemon.
            for name in names:
                subprocess.run(['docker', 'container', 'rm', '-f', name], capture_output=True, timeout=30)
            for kind, items in [('volume', volumes), ('network', networks), ('image', [image])]:
                for name in items:
                    subprocess.run(['docker', kind, 'rm', name], capture_output=True, timeout=30)
    print('Real Docker scope selection, running-container stop and shared-resource retention passed.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument('--installer', type=Path)
    group.add_argument('--binary', type=Path)
    args = parser.parse_args()
    native(args.installer.resolve()) if args.installer else docker(args.binary.resolve())
