"""Prepare public execution inputs consistently for shell, Python and jobs."""

from pathlib import Path
from wukong_worker.artifacts.common import public_file


def executable_inputs(root, argv, requested):
    """Only explicitly named argv files acquire executable copies."""
    if not isinstance(requested, list) or len(requested) > 16:
        raise ValueError("executable_inputs must contain at most 16 staged argv files")
    arguments = {str((root / value).resolve()) for value in argv if isinstance(value, str)}
    result = set()
    for value in requested:
        source = public_file(root, value)
        if not source.is_file() or str(source) not in arguments:
            raise ValueError("executable input must be a public file present in argv")
        result.add(str(source))
    return result


def stage_argv(root, argv, experiment, executable=()):
    from wukong_worker.platform import isolation

    staged = []
    for index, argument in enumerate(argv):
        path = Path(argument)
        absolute = (root / path).resolve()
        relative_input = not path.is_absolute() and argument.replace("\\", "/").split("/", 1)[
            0
        ] in {"files", "jobs", "fuzz"}
        if (path.is_absolute() and root in absolute.parents) or relative_input:
            if absolute.is_file():
                argument = str(
                    isolation.stage_file(
                        absolute, experiment, executable=index == 0 or str(absolute) in executable
                    )
                )
            elif absolute.is_dir():
                argument = str(
                    isolation.stage_directory(absolute, experiment, "directory-" + str(index))
                )
            else:
                raise ValueError(
                    "isolated jobs must write output relative to their fresh working directory"
                )
        staged.append(argument)
    return staged


def execution_metadata(cwd, experiment):
    return {
        "cwd": str(cwd),
        "fresh_working_directory": bool(experiment),
        "inputs": experiment.get("inputs", []) if experiment else [],
    }
