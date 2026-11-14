"""Actual Linux handlers and their readiness; wire types/effects come from Rust."""
from functools import partial
from dataclasses import dataclass
from worker_contract import METHODS


@dataclass(frozen=True)
class Registration:
    handler: object
    readiness: str
    capabilities: tuple


def capabilities(registrations, ready):
    result = {}
    for operation in registrations.values():
        for name in operation.capabilities:
            result[name] = ready[operation.readiness]
    return result


def register(runtime):
    registrations = {}
    def add(name, handler, readiness, capabilities=()):
        if name not in METHODS or name in registrations:
            raise ValueError("invalid or duplicate worker registration: " + name)
        registrations[name] = Registration(handler, readiness, capabilities)
    add('job.start', runtime.jobs.start, 'execution', ('process_jobs', 'process_stdin', 'launch_spec_jobs', 'persistent_jobs', 'native_target_execution'))
    add('crash.replay', partial(runtime.jobs.start_job, kind='replay'), 'execution', ('crash_replay',))
    add('poc.verify', partial(runtime.jobs.start_job, kind='verify'), 'execution', ('poc_verify',))
    add('py.run', runtime.jobs.python, 'execution', ('py_run',))
    add('fuzz.start', runtime.jobs.fuzz, 'fuzz', ('fuzz_qemu',))
    add('angr.solve', runtime.jobs.solve, 'symbolic', ('symbolic_execution',))
    add('input.minimize', partial(runtime.jobs.start_job, kind='minimize'), 'minimize', ('input_minimize_signal_only',))
    add('interact.start', runtime.sessions.interact_start, 'interactive', ('interactive_process',))
    add('debug.start', runtime.sessions.debug_start, 'debugger', ('debug_gdb_mi',))
    add('file.put', runtime.files.put, 'files')
    add('file.get', runtime.files.get, 'files')
    add('file.upload.begin', runtime.transfers.begin, 'files', ('resumable_file_transfer',))
    add('file.upload.write', runtime.transfers.write, 'files')
    add('file.upload.status', runtime.transfers.upload_status, 'files')
    add('file.upload.commit', runtime.transfers.commit, 'files')
    add('file.stat', runtime.transfers.stat, 'files')
    add('file.read', runtime.transfers.read, 'files')
    add('exec.run', runtime.commands.run, 'execution', ('shell_exec',))
    add('emu.run', runtime.emu.run, 'emulation', ('emulation',))
    add('libc.identify', runtime.libc.identify, 'elf', ('libc_identify',))
    add('runtime.profile', runtime.profile.profile, 'files', ('runtime_profile', 'mitigation_profile'))
    add('runtime.prepare', runtime.profile.prepare, 'elf', ('runtime_prepare',))
    add('cyclic.offset', runtime.crash.cyclic_offset, 'cyclic', ('cyclic_offset',))
    add('core.snapshot', runtime.crash.core_snapshot, 'core', ('core_snapshot',))
    add('seccomp.analyze', runtime.seccomp.analyze, 'files', ('seccomp_analyze',))
    add('network.profile', runtime.network.profile, 'network', ('network_poc_verify',))
    add('ping', runtime.ping, 'control')
    add('doctor', runtime.doctor, 'control')
    add('worker.shutdown', runtime.shutdown, 'control')
    add('ghidra.query', runtime.ghidra_query, 'ghidra', ('ghidra_query',))
    add('job.status', runtime.jobs.query, 'control')
    add('job.cancel', runtime.jobs.cancel, 'control')
    add('job.list', runtime.jobs.list, 'control')
    add('job.artifacts', runtime.jobs.artifacts, 'control')
    add('fuzz.status', runtime.jobs.query, 'control')
    add('fuzz.stop', runtime.jobs.cancel, 'control')
    add('interact.write', runtime.sessions.interact_write, 'control')
    add('interact.profile', runtime.sessions.interact_profile, 'control')
    add('interact.close', runtime.sessions.interact_close, 'control')
    add('debug.inspect', runtime.sessions.debug_inspect, 'debug_session')
    add('debug.control', runtime.sessions.debug_control, 'debug_session')
    add('debug.command', runtime.sessions.debug_command, 'debug_session')
    add('debug.profile', runtime.sessions.debug_profile, 'debug_session')
    add('debug.events', runtime.sessions.debug_events, 'debug_session')
    add('debug.snapshot', runtime.sessions.debug_snapshot, 'debug_session')
    add('debug.close', runtime.sessions.debug_close, 'control')
    return registrations
