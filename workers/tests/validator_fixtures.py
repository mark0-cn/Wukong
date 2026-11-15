"""Trusted, standalone protocol fixtures; never challenge attachments."""

from pathlib import Path
import shlex
import inspect
from wukong_analysis.validator import evaluate


def candidate(broken=False, shell=False):
    skill_path = Path(__file__).resolve().parents[2] / "prompts/support/validation.md"
    if skill_path.exists():
        evaluator = skill_path.read_text("utf-8").split("```python\n", 1)[1].split("```", 1)[0]
    else:
        evaluator = inspect.getsource(evaluate).replace("def evaluate(", "def validation_status(")
    if broken:
        evaluator += """
original_evaluator = validation_status
def validation_status(manifest, observations):
    # Reproduce the old defect: trust banners/comparisons, ignore child outcome.
    observations = copy.deepcopy(observations)
    for check in manifest:
        if check['kind'] == 'process' and check['id'] in observations:
            observations[check['id']].update(state='completed', exit_code=check['expected_exit_code'],
                                           signal=None, output_complete=True)
    return original_evaluator(manifest, observations)
"""
    program = (
        """import json, subprocess, sys, copy
from pathlib import Path
MANIFEST = [
 {'id':'accept','kind':'process','expected_exit_code':0,'required_stdout':['ACCEPT'],'forbidden_stdout':['REJECT']},
 {'id':'control','kind':'process','expected_exit_code':7,'required_stdout':['CONTROL'],'forbidden_stdout':['ACCEPT']},
 {'id':'comparison','kind':'comparison','expected':2}]
"""
        + evaluator
        + """
if '--validation-io' in sys.argv:
    request = json.load(sys.stdin)
    if request['operation'] == 'describe':
        print(json.dumps({'schema_version':1,'manifest':MANIFEST})); sys.exit(0)
    observations = request['observations']
else:
    observations = {}
    for name, code in [('accept', "print('ACCEPT')"), ('control', "print('CONTROL'); raise SystemExit(7)")]:
        result = subprocess.run([sys.executable,'-c',code],capture_output=True,text=True,timeout=2)
        observations[name] = {'state':'completed','exit_code':result.returncode,'signal':None,
                              'stdout':result.stdout,'output_complete':True}
    observations['comparison'] = {'observed':2}
    Path('baseline.json').write_text(json.dumps({'schema_version':1,'manifest':MANIFEST,'observations':observations}))
status = validation_status(MANIFEST, observations)
print(json.dumps({'status':status}))
sys.exit({'pass':0,'fail':1,'inconclusive':2}[status])
"""
    )
    return "#!/bin/sh\nexec python3 -c " + shlex.quote(program) + ' "$@"\n' if shell else program
