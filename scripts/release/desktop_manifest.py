"""Create the cross-platform updater catalog after image acceptance and signing."""
import argparse
from scripts._lib.checksums import sha256
import json
from pathlib import Path
import subprocess
import os
from scripts._lib.versioning import version
from scripts.release.release_notes import update_summary, validate


def generate(directory, validation, content_path):
    image = json.loads((validation / 'image.json').read_text(encoding='utf-8'))
    report = json.loads((validation / 'validation-summary.json').read_text(encoding='utf-8'))
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    number = version()
    content_bytes = content_path.read_bytes()
    content = validate(json.loads(content_bytes), revision, number)
    reference = image['repository'] + '@' + image['digest']
    if not report.get('passed') or report['source_revision'] != revision or image['source_revision'] != revision or report['image'] != reference:
        raise ValueError('Image acceptance identity mismatch')
    platforms = {}
    for target, pattern in [('windows-x86_64', '*.exe'), ('linux-x86_64', '*.AppImage')]:
        files = list(directory.glob(pattern))
        if len(files) != 1:
            raise ValueError('Expected exactly one installer for ' + target)
        file = files[0]
        platforms[target] = dict(url=f'https://github.com/mark0-cn/Wukong/releases/download/v{number}/{file.name}',
            signature=file.with_name(file.name + '.sig').read_text().strip(),
            size=file.stat().st_size, sha256=sha256(file))
    (directory / 'release-content.json').write_bytes(content_bytes)
    manifest = dict(schema=1, version=number, notes=update_summary(content), minimum_version='0.1.2',
        release_content_sha256=sha256(directory / 'release-content.json'),
        runtime_protocol=3, data_schema=5, image=reference, image_size=image['size_bytes'],
        source_revision=revision, platforms=platforms)
    output = directory / 'latest.json'
    output.write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
    release = dict(schema_version=3, version=number, source_revision=revision,
                   tool_platform='linux/amd64', platforms=list(platforms), image=reference,
                   image_id=image.get('image_id'), image_size_bytes=image['size_bytes'],
                   runtime_protocol=3, data_schema=5)
    (directory / 'release.json').write_text(json.dumps(release, indent=2) + '\n', encoding='utf-8')
    subprocess.run(['npx.cmd' if os.name == 'nt' else 'npx', '--no-install', 'tauri', 'signer', 'sign', str(output)], check=True)
    return manifest


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--directory', type=Path, required=True)
    p.add_argument('--validation', type=Path, required=True)
    p.add_argument('--content', type=Path, required=True)
    a = p.parse_args()
    generate(a.directory, a.validation, a.content)
