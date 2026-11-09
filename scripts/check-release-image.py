"""Exercise the published candidate via the host CLI; no model credentials required."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--client", type=Path, required=True)
    parser.add_argument("--docker", default="docker")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = str(args.client.resolve())
    workspace_id = None
    checks = {}

    def command(argv, timeout=300):
        result = subprocess.run(list(map(str, argv)), capture_output=True, text=True, timeout=timeout)
        if result.returncode:
            raise RuntimeError(f"Command failed: {argv[:4]}\n{result.stdout[-4000:]}\n{result.stderr[-4000:]}")
        return result.stdout

    def docker(*argv):
        return command([args.docker, *argv])

    def save(name, value):
        (args.output / name).write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")

    try:
        inspection = json.loads(docker("image", "inspect", args.image))[0]
        assert inspection["Os"] == "linux" and inspection["Architecture"] == "amd64"
        assert inspection["Config"]["Labels"]["org.opencontainers.image.revision"] == args.revision
        assert "@sha256:" in args.image, "Acceptance must use a registry digest"
        repository, digest = args.image.split("@", 1)
        image = dict(repository=repository, digest=digest, image_id=inspection["Id"],
                     source_revision=args.revision, platform="linux/amd64", size_bytes=inspection["Size"])
        save("image.json", image)
        with tempfile.TemporaryDirectory(prefix="re-agent-release-") as directory:
            root = Path(directory)
            config = root / "re-agent.toml"
            config.write_text('state_dir = ' + json.dumps(str(root / "assessments")) + '\n[runtime]\ndocker_executable = ' + json.dumps(args.docker) + '\n[workbench]\nroot = ' + json.dumps(str(root / "workbench")) + '\nimage = ' + json.dumps(args.image) + '\n', encoding="utf-8")

            def cli(*argv):
                return json.loads(command([client, "--config", config, *argv], timeout=600))

            status = cli("env", "status")
            workspace_id = status["id"]
            cli("env", "up")
            doctor = cli("env", "doctor")
            save("doctor.json", doctor)
            assert doctor["passed"], "Tool doctor failed"
            checks["doctor"] = True
            tool = f"re-agent-{workspace_id}-tools"
            output = docker("exec", "--user", "10001:10001", tool, "python3", "-m", "unittest", "discover", "-s", "/opt/re-agent/runtime", "-p", "test_workbench*.py", "-v")
            (args.output / "worker-tests.txt").write_text(output, encoding="utf-8")
            checks["launch_and_archive"] = True
            fixture = root / "fixture"
            fixture.mkdir()
            (fixture / "echo.c").write_text('#include <stdio.h>\nint main(){char b[64]; if(!fgets(b,sizeof b,stdin))return 1; printf("fixture:%s",b);return 0;}\n')
            (fixture / "Dockerfile").write_text(f'FROM {args.image}\nCOPY echo.c /srv/echo.c\nRUN gcc -g -o /srv/echo /srv/echo.c\nENTRYPOINT ["socat","TCP-LISTEN:1337,reuseaddr,fork","EXEC:/srv/echo"]\n')
            cli("challenge", "import", fixture, "--id", "release-check")
            manifest = root / "workbench/challenges/release-check/challenge.toml"
            manifest.write_text(manifest.read_text().replace('mode = "offline"', 'mode = "local_service"'))
            cli("challenge", "build", "release-check")
            for path, role in [("/srv/echo", "binary"), ("/lib/x86_64-linux-gnu/libc.so.6", "libc"), ("/lib64/ld-linux-x86-64.so.2", "loader")]:
                cli("challenge", "extract", "release-check", "--path", path, "--role", role)
            cli("challenge", "prepare", "release-check")
            cli("challenge", "up", "release-check")
            challenge_tool = f"re-agent-{workspace_id}-release-check-tools"
            request = "import socket; s=socket.create_connection(('challenge',1337),5); s.sendall(b'ci\\n'); assert s.recv(64)==b'fixture:ci\\n'"
            for attempt in range(20):
                try:
                    docker("exec", challenge_tool, "python3", "-c", request)
                    break
                except RuntimeError:
                    if attempt == 19:
                        raise
                    time.sleep(.2)
            checks["cli_import_build_extract_prepare_connect"] = True
            cli("challenge", "debug", "release-check")
            debug = f"re-agent-{workspace_id}-release-check-debug"
            attach = docker("exec", debug, "gdb", "-nx", "-nh", "-batch", "-ex", "set sysroot /proc/1/root", "-ex", "attach 1", "-ex", "info threads", "-ex", "detach")
            assert "Thread" in attach and "detached" in attach, attach
            (args.output / "debug-attach.txt").write_text(attach, encoding="utf-8")
            checks["debug_attach"] = True
            cli("challenge", "verify", "release-check")
            cli("challenge", "down", "release-check")
            # A distinct image ID catches upgrades that accidentally keep the old worker.
            update_dockerfile = fixture / "Dockerfile.update"
            update_dockerfile.write_text(f'FROM {args.image}\nLABEL re-agent.acceptance="{workspace_id}"\n')
            update_tag = "re-agent-release-update:" + workspace_id
            docker("build", "-f", update_dockerfile, "-t", update_tag, fixture)
            updated_id = json.loads(docker("image", "inspect", update_tag))[0]["Id"]
            assert updated_id != image["image_id"]
            cli("env", "update", "--image", update_tag)
            assert cli("env", "status")["environment"]["image_id"] == updated_id
            cli("env", "rollback")
            assert cli("env", "status")["environment"]["image_id"] == image["image_id"]
            assert cli("env", "doctor")["passed"]
            checks["update_rollback"] = True
        save("validation-summary.json", {"passed": True, "source_revision": args.revision, "image": args.image, "checks": checks, "windows_docker_desktop_e2e": "not covered by hosted runners"})
    except Exception as error:
        save("validation-summary.json", {"passed": False, "source_revision": args.revision, "checks": checks, "error": str(error)})
        raise
    finally:
        if workspace_id:
            label = "label=re-agent.workspace=" + workspace_id
            for kind, listing in [("container", ["ps", "-aq"]), ("network", ["network", "ls", "-q"]), ("volume", ["volume", "ls", "-q"])]:
                ids = docker(*listing, "--filter", label).split()
                if ids:
                    docker(kind, "rm", *(["-f", "-v"] if kind == "container" else []), *ids)


if __name__ == "__main__":
    main()
