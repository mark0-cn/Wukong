"""Build the host platform's standalone desktop installation file."""
import argparse
import platform
from pathlib import Path
import shutil
from scripts._lib.versioning import ROOT
from scripts.release.local_package import checkout_identity
from scripts.release.build_installer import build


def package(output, *, signed=False, check=False):
    system = platform.system()
    if system not in ('Windows', 'Linux') or platform.machine().lower() not in ('amd64', 'x86_64'):
        raise ValueError('Native packages support Windows x64 and Linux x64.')
    revision = checkout_identity()
    if output.exists():
        raise ValueError('Output already exists; choose a new --output directory.')
    if not shutil.which('cargo') or not shutil.which('npm.cmd' if system == 'Windows' else 'npm'):
        raise ValueError('Cargo and Node.js/npm must be on PATH.')
    if check:
        print('Source and tool entry points checked. System libraries and installer tools are checked during build.')
        return
    build('windows' if system == 'Windows' else 'linux', signed)
    if checkout_identity() != revision:
        raise ValueError('Source changed while building.')
    pattern = 'nsis/*-setup.exe' if system == 'Windows' else 'appimage/*.AppImage'
    installers = list((ROOT / 'target/release/bundle').glob(pattern))
    if len(installers) != 1:
        raise ValueError('Expected one installer; remove stale build outputs before retrying.')
    output.mkdir(parents=True)
    installer = installers[0]
    shutil.copy2(installer, output / installer.name)
    if signed:
        signature = installer.with_name(installer.name + '.sig')
        shutil.copy2(signature, output / signature.name)
    print('Native installer: ' + str(output / installer.name))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path('target/local-installers'))
    parser.add_argument('--signed', action='store_true')
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    package(ROOT / args.output, signed=args.signed, check=args.check)
