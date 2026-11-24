import re
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]

COMPATIBILITY_PINS = {
    "angr": "9.2.78",
    "ailment": "9.2.78",
    "archinfo": "9.2.102",
    "claripy": "9.2.101",
    "cle": "9.2.78",
    "pyvex": "9.2.131",
    "pycparser": "2.21",
    "bitstring": "3.1.9",
    "z3-solver": "4.13.4.0",
    "unicorn": "2.1.4",
    "protobuf": "5.29.4",
    "capstone": "5.0.6",
}


def canonical_name(name):
    return re.sub(r"[-_.]+", "-", name).lower()


def parse_image_lock(contents):
    entries = {}
    current = None
    for raw_line in contents.splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("--hash="):
            if current is None:
                raise AssertionError("hash without a requirement")
            hash_line = line.removesuffix("\\").rstrip()
            algorithm, digest = hash_line.removeprefix("--hash=").split(":", 1)
            if algorithm != "sha256" or not re.fullmatch(r"[0-9a-f]{64}", digest):
                raise AssertionError(f"invalid hash: {line}")
            entries[current][1].append(digest)
            continue
        match = re.fullmatch(r"([A-Za-z0-9_.-]+)==([^\s\\]+)\s*\\", line)
        if match is None:
            raise AssertionError(f"requirement is not exactly pinned and continued: {raw_line}")
        name = canonical_name(match.group(1))
        if name in entries:
            raise AssertionError(f"duplicate requirement: {name}")
        entries[name] = (match.group(2), [])
        current = name
    return entries


def read_image_lock():
    contents = (ROOT / "workers/requirements.txt").read_text(encoding="utf-8")
    return parse_image_lock(contents)


class PythonDependencyTests(unittest.TestCase):
    def test_image_lock_accepts_continued_hash_lines(self):
        first_hash = "a" * 64
        second_hash = "b" * 64
        entries = parse_image_lock(
            "example==1.2.3 \\\n"
            f"    --hash=sha256:{first_hash} \\\n"
            f"    --hash=sha256:{second_hash}\n"
        )
        self.assertEqual(entries, {"example": ("1.2.3", [first_hash, second_hash])})

    def test_image_lock_pins_and_hashes_direct_and_transitive_requirements(self):
        entries = read_image_lock()
        for name, (_, hashes) in entries.items():
            with self.subTest(name=name):
                self.assertTrue(hashes)

        runtime_requirements = (ROOT / "workers/runtime/requirements.txt").read_text(
            encoding="utf-8"
        )
        runtime_names = {
            canonical_name(line.split("==", 1)[0])
            for line in runtime_requirements.splitlines()
            if line and not line.startswith("#")
        }
        self.assertLessEqual(runtime_names, entries.keys())
        self.assertLessEqual(
            {
                "angr",
                "pwntools",
                "ropper",
                "z3-solver",
                "decorator",
                "pysmt",
                "cppheaderparser",
                "gitpython",
                "gitdb",
                "smmap",
                "setuptools",
            },
            entries.keys(),
        )
        for name, version in COMPATIBILITY_PINS.items():
            self.assertEqual(entries[name][0], version)

    def test_images_install_only_the_complete_hashed_python_lock(self):
        dockerfile = (ROOT / "workers/Dockerfile").read_text(encoding="utf-8")
        workbench = (ROOT / "workers/Dockerfile.workbench").read_text(encoding="utf-8")
        self.assertIn("COPY workers/requirements.txt", dockerfile)
        for option in (
            "--require-hashes",
            "--force-reinstall",
            "--no-deps",
            "--no-build-isolation",
        ):
            self.assertIn(option, dockerfile)
        for contents in (dockerfile, workbench):
            self.assertNotIn("workers/runtime/requirements.txt", contents)

    def test_ci_and_image_use_the_same_pinned_pyelftools(self):
        ci_requirements = (ROOT / "scripts/ci/requirements.txt").read_text(encoding="utf-8")
        match = re.search(r"^pyelftools==([^\s]+)$", ci_requirements, re.MULTILINE)
        self.assertIsNotNone(match)
        self.assertEqual(match.group(1), read_image_lock()["pyelftools"][0])
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertNotRegex(workflow, r"pip install[^\n]*\spyelftools(?:\s|$)")

    def test_dependabot_covers_the_image_lock(self):
        dependabot = (ROOT / ".github/dependabot.yml").read_text(encoding="utf-8")
        self.assertRegex(
            dependabot,
            r"package-ecosystem: pip\s+directory: /workers\s+schedule:",
        )
        group = re.search(
            r"compatible-image-python:(.*?)image-python-security:",
            dependabot,
            re.DOTALL,
        )
        self.assertIsNotNone(group)
        excluded = set(re.findall(r"^\s+-\s*'([^']+)'$", group.group(1), re.MULTILINE))
        self.assertLessEqual(COMPATIBILITY_PINS.keys(), excluded)


if __name__ == "__main__":
    unittest.main()
