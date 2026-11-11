import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from scripts.release.desktop_manifest import generate


class DesktopManifestTests(unittest.TestCase):
    def test_installer_and_runtime_share_the_trust_anchor(self):
        root = Path(__file__).resolve().parents[2]
        config = json.loads((root / 'tauri.conf.json').read_text(encoding='utf-8'))
        self.assertEqual(config['plugins']['updater']['pubkey'],
                         (root / 'src/desktop/update-public.key').read_text().strip())
    def test_requires_both_platforms_and_matching_acceptance(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            image = dict(repository='hlmark0/wukong', digest='sha256:' + 'a'*64,
                         source_revision='b'*40, size_bytes=123)
            (root / 'image.json').write_text(json.dumps(image))
            report = dict(passed=True, source_revision='b'*40,
                          image=image['repository'] + '@' + image['digest'])
            (root / 'validation-summary.json').write_text(json.dumps(report))
            content = root / 'content.json'
            content.write_text(json.dumps(dict(schema=1, tag='v0.2.0', version='0.2.0',
                source_revision='b'*40, entries=[], changelog='Technical changes')))
            with patch('scripts.release.desktop_manifest.subprocess.check_output', return_value='b'*40), \
                 patch('scripts.release.desktop_manifest.version', return_value='0.2.0'), \
                 patch('scripts.release.desktop_manifest.subprocess.run') as sign:
                with self.assertRaises(ValueError):
                    generate(root, root, content)
                for name in ['Wukong_setup.exe', 'Wukong.AppImage']:
                    (root / name).write_bytes(b'installer')
                    (root / (name + '.sig')).write_text('signature')
                manifest = generate(root, root, content)
                self.assertEqual(set(manifest['platforms']), {'windows-x86_64', 'linux-x86_64'})
                self.assertEqual(manifest['image'], report['image'])
                self.assertIn('release_content_sha256', manifest)
                self.assertEqual((root / 'release-content.json').read_bytes(), content.read_bytes())
                sign.assert_called_once()
                report['source_revision'] = 'c'*40
                (root / 'validation-summary.json').write_text(json.dumps(report))
                with self.assertRaises(ValueError):
                    generate(root, root, content)
