"""Processes."""
from pathlib import Path
import os
import signal
import subprocess
import sys
import threading
import time
from worker_common import MAX_FILE, tail

def detached_options():
    if os.name == "nt":
        return {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.CREATE_NO_WINDOW}
    return {"start_new_session": True}

def attach_windows_job(process):
    """A kill-on-close Job Object owns descendants, including orphaned children."""
    if os.name != "nt":
        return
    import ctypes
    from ctypes import wintypes
    class BasicLimits(ctypes.Structure):
        _fields_ = [("ProcessTime", ctypes.c_longlong), ("JobTime", ctypes.c_longlong), ("Flags", wintypes.DWORD),
                    ("MinimumWorkingSet", ctypes.c_size_t), ("MaximumWorkingSet", ctypes.c_size_t), ("ActiveProcesses", wintypes.DWORD),
                    ("Affinity", ctypes.c_size_t), ("Priority", wintypes.DWORD), ("Scheduling", wintypes.DWORD)]
    class IoCounters(ctypes.Structure):
        _fields_ = [(name, ctypes.c_ulonglong) for name in ["ReadOps", "WriteOps", "OtherOps", "ReadBytes", "WriteBytes", "OtherBytes"]]
    class ExtendedLimits(ctypes.Structure):
        _fields_ = [("Basic", BasicLimits), ("Io", IoCounters), ("ProcessMemory", ctypes.c_size_t), ("JobMemory", ctypes.c_size_t),
                    ("PeakProcessMemory", ctypes.c_size_t), ("PeakJobMemory", ctypes.c_size_t)]
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateJobObjectW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR]
    kernel.CreateJobObjectW.restype = wintypes.HANDLE
    kernel.SetInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD]
    kernel.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = kernel.CreateJobObjectW(None, None)
    limits = ExtendedLimits()
    limits.Basic.Flags = 0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    if handle and kernel.SetInformationJobObject(handle, 9, ctypes.byref(limits), ctypes.sizeof(limits)) and kernel.AssignProcessToJobObject(handle, int(process._handle)):
        process._wukong_job = (kernel, handle)
    elif handle:
        kernel.CloseHandle(handle)

def close_windows_job(process):
    job = getattr(process, "_wukong_job", None)
    if job:
        job[0].CloseHandle(job[1])
        process._wukong_job = None
        return True
    return False

def process_identity(pid):
    """Creation identity, not PID alone. Unknown access is not proof of death."""
    if not pid:
        return "unknown"
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        handle = kernel.OpenProcess(0x1000, False, int(pid))
        if not handle:
            return "missing" if ctypes.get_last_error() == 87 else "unknown"
        try:
            times = [wintypes.FILETIME() for _ in range(4)]
            if kernel.GetProcessTimes(handle, *(ctypes.byref(value) for value in times)):
                return "windows:" + str((times[0].dwHighDateTime << 32) | times[0].dwLowDateTime)
            return "unknown"
        finally:
            kernel.CloseHandle(handle)
    if sys.platform.startswith("linux"):
        try:
            stat = Path("/proc") / str(int(pid)) / "stat"
            fields = stat.read_text().rsplit(")", 1)[1].split()
            if fields[0] == "Z":
                return "missing"
            return "linux:" + fields[19]
        except FileNotFoundError:
            return "missing"
        except (OSError, IndexError):
            return "unknown"
    try:
        os.kill(int(pid), 0)
        return "unknown"
    except ProcessLookupError:
        return "missing"
    except PermissionError:
        return "unknown"

def terminate_tree(process):
    if close_windows_job(process):
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
        return
    if os.name == "nt":
        if process.poll() is not None:
            return
        subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       creationflags=subprocess.CREATE_NO_WINDOW, timeout=15)
    else:
        try:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=1.5)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            pass
        # A reaped leader does not mean its descendants have exited. Kill
        # the remaining group, including children that ignored SIGTERM.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()

def run_process(argv, cwd, env, timeout_secs, output_limit, log_prefix, cancel_file=None, experiment=None, stdin_bytes=None):
    if not isinstance(argv, list) or not argv or len(argv) > 128 or any(not isinstance(a, str) or "\0" in a or len(a) > 65536 for a in argv):
        raise ValueError("argv must be an array of 1..128 bounded strings")
    if stdin_bytes is not None and len(stdin_bytes) > MAX_FILE:
        raise ValueError("stdin payload exceeds 32 MiB")
    start = time.monotonic()
    credential_options = {}
    if experiment:
        import isolation
        credential_options = isolation.credentials(experiment)
        env = {**env, "HOME": experiment["work"], "TMPDIR": experiment["work"]}
    process = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.PIPE if stdin_bytes is not None else subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, **detached_options(), **credential_options)
    attach_windows_job(process)
    counts = {"stdout": 0, "stderr": 0}

    def feed_stdin():
        # Bounded best-effort write: a target that never reads must not wedge
        # the call; the timeout and cancel paths own termination.
        try:
            process.stdin.write(stdin_bytes)
            process.stdin.flush()
        except (BrokenPipeError, OSError):
            pass
        try:
            process.stdin.close()
        except OSError:
            pass

    if stdin_bytes is not None:
        threading.Thread(target=feed_stdin, daemon=True).start()

    def drain(name, stream):
        with open(str(log_prefix) + "." + name, "wb") as log:
            while True:
                data = stream.read(8192)
                if not data:
                    break
                remaining = max(0, output_limit - counts[name])
                if remaining:
                    log.write(data[:remaining])
                counts[name] += len(data)
        stream.close()

    readers = [threading.Thread(target=drain, args=(name, getattr(process, name)), daemon=True) for name in counts]
    for thread in readers:
        thread.start()
    state = "completed"
    while process.poll() is None:
        if cancel_file and Path(cancel_file).exists():
            state = "cancelled"
            terminate_tree(process)
            break
        if time.monotonic() - start >= timeout_secs:
            state = "timed_out"
            terminate_tree(process)
            break
        time.sleep(0.03)
    # A completed parent must not leave descendants holding output pipes open.
    if os.name == "nt":
        close_windows_job(process)
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    for thread in readers:
        thread.join(timeout=5)
    return {"state": state, "exit_code": process.returncode,
            "signal": -process.returncode if process.returncode is not None and process.returncode < 0 and os.name != "nt" else None,
            "duration_secs": round(time.monotonic() - start, 4),
            "stdout": tail(str(log_prefix) + ".stdout"), "stderr": tail(str(log_prefix) + ".stderr"),
            "output_bytes": counts, "output_truncated": any(v > output_limit for v in counts.values())}
