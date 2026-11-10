from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from scripts.release import native_package


class NativePackageTests(unittest.TestCase):
    def test_existing_output_cannot_be_overwritten(self):
        with tempfile.TemporaryDirectory() as tmp, \
             patch.object(native_package.platform, 'system', return_value='Windows'), \
             patch.object(native_package.platform, 'machine', return_value='AMD64'), \
             patch.object(native_package, 'checkout_identity', return_value='a'*40), \
             patch.object(native_package, 'build') as build:
            with self.assertRaisesRegex(ValueError, 'Output already exists'):
                native_package.package(Path(tmp))
            build.assert_not_called()

    def test_preflight_never_builds_or_fetches_images(self):
        with tempfile.TemporaryDirectory() as tmp, \
             patch.object(native_package.platform, 'system', return_value='Linux'), \
             patch.object(native_package.platform, 'machine', return_value='x86_64'), \
             patch.object(native_package, 'checkout_identity', return_value='a'*40), \
             patch.object(native_package.shutil, 'which', return_value='/bin/tool'), \
             patch.object(native_package, 'build') as build:
            output = Path(tmp)/'output'
            native_package.package(output, check=True)
            build.assert_not_called()
            self.assertFalse(output.exists())
