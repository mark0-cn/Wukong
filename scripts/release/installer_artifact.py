"""Stage signed native installers and exercise their actual installation format."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from scripts._lib.versioning import ROOT, version


def stage(target, output):
    pattern = 'nsis/*-setup.exe' if target == 'windows' else 'appimage/*.AppImage'
    files = list((ROOT / 'target/release/bundle').glob(pattern))
    if len(files) != 1:
        raise ValueError('Expected one native installer')
    source = files[0]
    signature = source.with_name(source.name + '.sig')
    if not signature.is_file():
        raise ValueError('Installer signature is missing')
    with tempfile.TemporaryDirectory(prefix='wukong-install-') as tmp:
        destination = Path(tmp) / 'application'
        if target == 'windows':
            subprocess.run([str(source), '/S', '/D=' + str(destination)], check=True, timeout=180)
            binary = destination / 'wukong.exe'
            result = subprocess.check_output([str(binary), '--version'], text=True, timeout=30)
            if version() not in result:
                raise ValueError('Installed executable version mismatch')
            uninstall = destination / 'uninstall.exe'
            if not uninstall.exists():
                raise ValueError('Installer did not create an uninstaller')
            subprocess.run([str(uninstall), '/S', '_?=' + str(destination)], check=True, timeout=180)
        else:
            source.chmod(0o755)
            env = dict(os.environ, APPIMAGE_EXTRACT_AND_RUN='1', XDG_DATA_HOME=str(Path(tmp) / 'data'))
            result = subprocess.check_output([str(source), '--version'], text=True, env=env, timeout=60)
            if version() not in result:
                raise ValueError('AppImage executable version mismatch')
            # Exercise a real webview, without requiring Docker or a model account.
            result = subprocess.run(['xvfb-run', '-a', 'timeout', '20', str(source)], env=env, capture_output=True, text=True, timeout=45)
            if result.returncode != 124:
                raise ValueError('AppImage desktop exited before smoke-test timeout: ' + result.stderr[-4000:])
    output.mkdir(parents=True, exist_ok=True)
    for file in [source, signature]:
        shutil.copy2(file, output / file.name)


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--target', required=True, choices=['windows', 'linux'])
    p.add_argument('--output', required=True, type=Path)
    a = p.parse_args()
    stage(a.target, a.output)
