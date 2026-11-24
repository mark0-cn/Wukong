"""Concrete job-kind bindings, assembled outside the resource lifecycle."""

from wukong_worker.operations.process_job import execute as process
from wukong_worker.operations.verify_job import execute as verify
from wukong_worker.operations.reduce_job import execute as reduce
from wukong_worker.operations.trace_job import execute as trace

EXECUTORS = {
    "process": process,
    "fuzz": process,
    "replay": verify,
    "verify": verify,
    "minimize": reduce,
    "trace": trace,
}


def execute_job(root, directory, spec):
    try:
        execute = EXECUTORS[spec["kind"]]
    except KeyError as error:
        raise ValueError("unknown job kind") from error
    return execute(root, directory, spec)
