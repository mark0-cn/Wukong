import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from scripts._lib.repository import ROOT
from scripts.worker.layout import install_python_components


class WindowsPythonComponents(unittest.TestCase):
    def test_explicit_layout_runs_analysis_without_linux_worker_or_site_packages(self):
        with tempfile.TemporaryDirectory() as temporary:
            runtime = Path(temporary) / "runtime"
            (runtime / "python").mkdir(parents=True)
            (runtime / "python/python313._pth").write_text("old-worker-path\n")
            install_python_components(runtime)
            inventory = json.loads((ROOT / "workers/windows/python-components.json").read_text())
            package = runtime / "python/Lib/site-packages/wukong_analysis"
            self.assertEqual(sorted(path.name for path in package.iterdir()), sorted(inventory["analysis"]))
            self.assertFalse((runtime / "worker").exists())
            self.assertEqual((runtime / "python/python313._pth").read_bytes(), b"python313.zip\n.\nLib/site-packages\n")
            self.assertFalse(list(runtime.rglob("worker_*.py")))
            for name in inventory["analysis"]:
                self.assertEqual((package / name).read_bytes(), (ROOT / "workers/analysis/wukong_analysis" / name).read_bytes())
            source = (
                "import json,sys;sys.path.insert(0,sys.argv[1]);"
                "from wukong_analysis.seccomp import analyze;"
                "result=analyze(bytes.fromhex('060000000000ff7f'),probes=[{'arch':0xc000003e,'nr':1}]);"
                "assert not any(name.startswith('worker_') for name in sys.modules);"
                "print(json.dumps(result))"
            )
            result = subprocess.run([sys.executable, "-I", "-S", "-c", source, str(package.parent)],
                                    check=True, capture_output=True, text=True, timeout=15)
            self.assertEqual(json.loads(result.stdout)["probes"][0]["decision"]["action"], "ALLOW")


if __name__ == "__main__":
    unittest.main()
