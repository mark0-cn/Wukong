"""Trusted language-neutral driver, executed as an owned isolated Python job."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import threading
from wukong_analysis.validator import scenarios


def invoke(command, request):
    proc = None
    try:
        proc = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        captured = {'stdout': bytearray(), 'stderr': bytearray()}
        overflow = threading.Event()
        def read(name):
            with getattr(proc, name) as stream:
                while chunk := stream.read(4096):
                    captured[name].extend(chunk[:max(0, 65537 - len(captured[name]))])
                    if len(captured[name]) > 65536:
                        overflow.set()
                        proc.kill()
                        return
        def write():
            try:
                proc.stdin.write(json.dumps(request).encode())
                proc.stdin.flush()
            except (BrokenPipeError, OSError):
                pass
            finally:
                proc.stdin.close()
        readers = [threading.Thread(target=read, args=(name,), daemon=True) for name in captured]
        for thread in readers:
            thread.start()
        threading.Thread(target=write, daemon=True).start()
        proc.wait(timeout=3)
        for thread in readers:
            thread.join(timeout=0.2)
        if overflow.is_set() or any(t.is_alive() for t in readers):
            return {'exit_code': proc.returncode, 'error': 'protocol output exceeds 64 KiB'}
        return {'exit_code': proc.returncode, 'response': json.loads(captured['stdout']), 'stderr': captured['stderr'].decode('utf-8', 'replace')}
    except (subprocess.TimeoutExpired, OSError, ValueError) as error:
        return {'error': type(error).__name__}
    finally:
        if proc is not None and proc.poll() is None:
            proc.kill()
            proc.wait(timeout=1)


def run(interpreter, candidate, baseline_path, arguments):
    baseline = json.loads(Path(baseline_path).read_text('utf-8'))
    if baseline.get('schema_version') != 1:
        raise ValueError('unsupported baseline schema')
    manifest, observations = baseline['manifest'], baseline['observations']
    cases = scenarios(manifest, observations)
    command = [interpreter, candidate, '--validation-io', *arguments]
    description = invoke(command, {'schema_version': 1, 'operation': 'describe'})
    described = description.get('exit_code') == 0 and description.get('response') == {
        'schema_version': 1, 'manifest': manifest}
    results = []
    for name, values, expected in cases if described else []:
        observed = invoke(command, {'schema_version': 1, 'operation': 'evaluate', 'observations': values})
        response = observed.get('response', {})
        passed = (observed.get('exit_code') == {'pass': 0, 'fail': 1, 'inconclusive': 2}[expected]
                  and isinstance(response, dict) and response.get('status') == expected)
        results.append({'scenario': name, 'expected': expected, 'observed': observed, 'passed': passed})
    return {'schema_version': 1, 'profile': 'self_check_v1',
            'artifact_sha256': hashlib.sha256(Path(candidate).read_bytes()).hexdigest(),
            'baseline_sha256': hashlib.sha256(Path(baseline_path).read_bytes()).hexdigest(),
            'description': description, 'cases': results,
            'passed': described and len(results) == len(cases) and all(r['passed'] for r in results),
            'limitation': 'Simulated predicate checks only; real --verify and collector/evaluator wiring require independent review.'}


if __name__ == '__main__':
    try:
        report = run(*sys.argv[1:4], sys.argv[4:])
    except Exception as error:
        report = {'schema_version': 1, 'passed': False, 'error': str(error)[:1024]}
    Path('validation-report.json').write_text(json.dumps(report), encoding='utf-8')
    print(json.dumps({key: value for key, value in report.items() if key not in ('cases', 'description')}))
    raise SystemExit(0 if report['passed'] else 1)
