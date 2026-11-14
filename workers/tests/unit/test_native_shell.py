"""Trusted CMD regression fixtures; containment is tested on the dedicated VM."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


@unittest.skipUnless(os.name == "nt", "requires Windows CMD")
class NativeShellTests(unittest.TestCase):
    def test_explicit_streams_and_drive_cwd_survive_extended_resource_paths(self):
        adapter = Path(__file__).resolve().parents[2] / "windows/native/shell_command.py"
        for extended in (False, True):
            with self.subTest(extended=extended), tempfile.TemporaryDirectory(prefix="native shell ") as temporary:
                state = Path(temporary)
                configuration = dict(schema=1, command='cd&&echo %AFL_TEST%&&echo native-stderr 1>&2&&"%SystemRoot%\\System32\\findstr.exe" /x native-stdin',
                                     cwd="work/space directory", env=dict(AFL_TEST="native-stdout"))
                (state / "command.json").write_text(json.dumps(configuration), encoding="utf-8")
                cwd = ("\\\\?\\" if extended else "") + str(state)
                result = subprocess.run([sys.executable, "-I", "-B", str(adapter), "command.json"],
                                        cwd=cwd, input=b"native-stdin\r\n", capture_output=True, timeout=10,
                                        creationflags=subprocess.CREATE_NO_WINDOW)
                self.assertEqual(result.returncode, 0, result.stderr)
                lines = result.stdout.decode("utf-8").splitlines()
                self.assertEqual(len(lines), 3, result.stdout)
                self.assertEqual(os.path.normcase(lines[0]), os.path.normcase(str(state / configuration["cwd"])))
                self.assertEqual(lines[1], "native-stdout")
                self.assertEqual(lines[2], "native-stdin")
                self.assertEqual(result.stderr.strip(), b"native-stderr")


if __name__ == "__main__":
    unittest.main()
