"""Pure self_check_v1 predicate semantics and bounded perturbation vectors."""
import copy
import json


def evaluate(manifest, observations):
    """Reference semantics; incomplete observations take precedence over rejection."""
    failed, incomplete = False, False
    if not isinstance(manifest, list) or not 1 <= len(manifest) <= 32:
        raise ValueError("manifest requires 1..32 checks")
    if len(json.dumps(manifest)) > 65536:
        raise ValueError("manifest exceeds 64 KiB")
    identifiers = [check['id'] for check in manifest]
    if len(set(identifiers)) != len(identifiers) or not all(isinstance(x, str) and x for x in identifiers):
        raise ValueError("check ids must be unique nonempty strings")
    if not isinstance(observations, dict):
        raise ValueError("observations must be an object")
    for check in manifest:
        observation = observations.get(check['id'])
        if not isinstance(observation, dict):
            incomplete = True
            continue
        if check['kind'] == 'process':
            expected = check['expected_exit_code']
            if type(expected) is not int or expected < 0:
                raise ValueError("expected_exit_code must be a nonnegative integer")
            required, forbidden = check['required_stdout'], check['forbidden_stdout']
            if not all(isinstance(items, list) and len(items) <= 8 and all(isinstance(x, str) and 0 < len(x) <= 4096 for x in items)
                       for items in (required, forbidden)):
                raise ValueError("stdout predicates must be lists of nonempty strings")
            if (observation.get('state') != 'completed' or observation.get('output_complete') is not True
                    or type(observation.get('exit_code')) is not int
                    or 'signal' not in observation
                    or not isinstance(observation.get('stdout'), str)):
                incomplete = True
                continue
            failed |= (observation['exit_code'] != expected or observation['signal'] is not None
                       or any(text not in observation['stdout'] for text in required)
                       or any(text in observation['stdout'] for text in forbidden))
        elif check['kind'] == 'comparison':
            if 'observed' not in observation:
                incomplete = True
            else:
                failed |= json.dumps(observation['observed'], sort_keys=True, allow_nan=False) != json.dumps(check['expected'], sort_keys=True, allow_nan=False)
        else:
            raise ValueError("unsupported check kind")
    return 'inconclusive' if incomplete else 'fail' if failed else 'pass'


def scenarios(manifest, baseline):
    if evaluate(manifest, baseline) != 'pass':
        raise ValueError("baseline must be complete and satisfy every declared check")
    result = [('normal', baseline, 'pass')]
    for check in manifest:
        key = check['id']
        absent = copy.deepcopy(baseline)
        del absent[key]
        result.append((key + ':missing_check', absent, 'inconclusive'))
        if check['kind'] == 'process':
            for name, patch, expected in [
                ('abnormal_exit', {'exit_code': check['expected_exit_code'] + 1}, 'fail'),
                ('signal_after_output', {'exit_code': -11, 'signal': 11}, 'fail'),
                ('timeout_after_output', {'state': 'timed_out'}, 'inconclusive'),
                ('truncated_output', {'output_complete': False}, 'inconclusive'),
                ('missing_exit_code', {'exit_code': None}, 'inconclusive'),
            ]:
                altered = copy.deepcopy(baseline)
                altered[key].update(patch)
                result.append((key + ':' + name, altered, expected))
            for text in check['required_stdout']:
                altered = copy.deepcopy(baseline)
                altered[key]['stdout'] = altered[key]['stdout'].replace(text, '')
                result.append((key + ':missing_required_output', altered, 'fail'))
            for text in check['forbidden_stdout']:
                altered = copy.deepcopy(baseline)
                altered[key]['stdout'] += text
                result.append((key + ':contradictory_output', altered, 'fail'))
        else:
            altered = copy.deepcopy(baseline)
            altered[key]['observed'] = {'contradictory': check['expected']}
            result.append((key + ':contradictory_comparison', altered, 'fail'))
    if len(result) > 256:
        raise ValueError("validation exceeds 256 bounded cases; narrow the manifest")
    return result
