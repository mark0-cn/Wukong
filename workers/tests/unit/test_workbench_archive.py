import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from workbench_archive import export,restore

class ArchiveTests(unittest.TestCase):
    def test_restore_does_not_resurrect_process_authority(self):
        with tempfile.TemporaryDirectory() as d:
            base=Path(d);source=base/"source";source.mkdir()
            state=source/"debug/id/session.json";state.parent.mkdir(parents=True)
            state.write_text(json.dumps({"state":"running","gdb_pid":1234,"daemon_pid":2345}))
            archive=base/"state.tar";export(source,archive)
            destination=base/"destination";destination.mkdir();restore(destination,archive)
            value=json.loads((destination/"debug/id/session.json").read_text())
            self.assertEqual(value["state"],"lost");self.assertNotIn("gdb_pid",value)
    def test_traversal_archive_is_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);archive=root/"bad.tar"
            with tarfile.open(archive,"w") as stream:
                info=tarfile.TarInfo("../escape");info.size=4;stream.addfile(info,io.BytesIO(b"evil"))
            destination=root/"destination";destination.mkdir()
            with self.assertRaises(tarfile.FilterError):restore(destination,archive)
            self.assertFalse((root/"escape").exists())

if __name__=="__main__":unittest.main()
