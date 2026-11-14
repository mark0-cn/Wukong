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
    def add(service, readiness, names, capabilities=()):
        for name in names:
            if name not in METHODS or name in registrations:
                raise ValueError("invalid or duplicate worker registration: " + name)
            registrations[name] = Registration(partial(service, name), readiness, capabilities)
    add(runtime.jobs.dispatch, 'execution', ('job.start', 'crash.replay', 'poc.verify', 'py.run'), ('process_jobs', 'process_stdin', 'launch_spec_jobs', 'persistent_jobs', 'native_target_execution', 'crash_replay', 'poc_verify', 'py_run'))
    add(runtime.jobs.dispatch, 'fuzz', ('fuzz.start',), ('fuzz_qemu',))
    add(runtime.jobs.dispatch, 'symbolic', ('angr.solve',), ('symbolic_execution',))
    add(runtime.jobs.dispatch, 'minimize', ('input.minimize',), ('input_minimize_signal_only',))
    add(runtime.sessions.dispatch, 'interactive', ('interact.start',), ('interactive_process',))
    add(runtime.sessions.dispatch, 'debugger', ('debug.start',), ('debug_gdb_mi',))
    add(runtime.files.dispatch, 'files', ('file.put', 'file.get'), ('resumable_file_transfer',))
    add(runtime.transfers.dispatch, 'files', ('file.upload.begin', 'file.upload.write', 'file.upload.status', 'file.upload.commit', 'file.stat', 'file.read'), ('resumable_file_transfer',))
    add(runtime.commands.dispatch, 'execution', ('exec.run',), ('shell_exec',))
    add(runtime.emu.dispatch, 'emulation', ('emu.run',), ('emulation',))
    add(runtime.libc.dispatch, 'elf', ('libc.identify',), ('libc_identify',))
    add(runtime.profile.dispatch, 'files', ('runtime.profile',), ('runtime_profile', 'mitigation_profile'))
    add(runtime.profile.dispatch, 'elf', ('runtime.prepare',), ('runtime_prepare',))
    add(runtime.crash.dispatch, 'cyclic', ('cyclic.offset',), ('cyclic_offset',))
    add(runtime.crash.dispatch, 'core', ('core.snapshot',), ('core_snapshot',))
    add(runtime.seccomp.dispatch, 'files', ('seccomp.analyze',), ('seccomp_analyze',))
    add(runtime.network.dispatch, 'network', ('network.profile',), ('network_poc_verify',))
    add(runtime.control, 'control', ('ping', 'doctor', 'worker.shutdown'), ())
    add(runtime.control, 'ghidra', ('ghidra.query',), ('ghidra_query',))
    add(runtime.jobs.dispatch, 'control', ('job.status', 'job.cancel', 'job.list', 'job.artifacts', 'fuzz.status', 'fuzz.stop'))
    add(runtime.sessions.dispatch, 'control', ('interact.write', 'interact.profile', 'interact.close'))
    add(runtime.sessions.dispatch, 'debug_session', ('debug.inspect', 'debug.control', 'debug.command', 'debug.profile', 'debug.events', 'debug.snapshot', 'debug.close'))
    return registrations
