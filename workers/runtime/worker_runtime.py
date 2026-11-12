"""Worker composition and method routing."""
import os
import sys
from worker_capabilities import doctor
from worker_commands import CommandService
from worker_common import PROTOCOL, TERMINAL
from worker_emu import EmuService
from worker_files import FileService
from worker_jobs import JobManager
from worker_libc import LibcService
from worker_seccomp import SeccompService
from worker_network import NetworkService
from worker_profile import ProfileService
from worker_crash import CrashService
from worker_operations import OperationLedger
from worker_sessions import SessionManager
from worker_transfers import TransferService
from worker_identity import WorkerIdentity, ExclusiveLease

class Runtime:
    def __init__(self, root, trusted):
        self.root, self.trusted = root.resolve(), trusted
        self.identity = WorkerIdentity(self.root)
        self.isolated = sys.platform.startswith("linux") and os.geteuid() == 0 and not trusted
        self.jobs = JobManager(self.root, trusted, self.isolated)
        self.sessions = SessionManager(self.root, trusted, self.isolated)
        self.files = FileService(self.root, trusted, self.isolated)
        self.commands = CommandService(self.root, trusted, self.isolated)
        self.emu = EmuService(self.root, trusted, self.isolated)
        self.libc = LibcService(self.root, trusted, self.isolated)
        self.profile = ProfileService(self.root, trusted, self.isolated)
        self.crash = CrashService(self.root, trusted, self.isolated)
        self.seccomp = SeccompService(self.root, trusted, self.isolated)
        self.network = NetworkService(self.root, trusted, self.isolated)
        self.operations = OperationLedger(self.root)
        self.transfers = TransferService(self.root)
        self.lease = ExclusiveLease(self.root,self.identity,self.active_resources)
        self.handlers = {}
        for service in [self.jobs, self.sessions, self.files, self.commands, self.emu, self.libc, self.profile, self.crash, self.seccomp, self.network, self.transfers]:
            for name in service.methods:
                if name in self.handlers:
                    raise ValueError("duplicate worker method: " + name)
                self.handlers[name] = service.dispatch

    def dispatch(self, method, params):
        if method.startswith('environment.lease.'):
            self.identity.validate(params.get('_binding'))
            if method=='environment.lease.acquire': return self.lease.acquire(params)
            if method=='environment.lease.release': return self.lease.release(params)
            raise ValueError('unsupported environment lease operation')
        if method not in {'ping','doctor'}:
            if os.name=='nt' and not self.trusted:
                return self.lease.dispatch(params,lambda clean:self.operations.invoke(method,clean,self.dispatch_inner,self.sessions.reconcile_reply))
            if '_binding' in params:
                self.identity.validate(params['_binding'])
                params={key:value for key,value in params.items() if key!='_binding'}
        return self.operations.invoke(method, params, self.dispatch_inner, self.sessions.reconcile_reply)

    def active_resources(self):
        return (any(session.process.poll() is None or (getattr(session,'inferior',None) is not None and session.inferior.poll() is None) for session in self.sessions.debug.values())
                or any(session.process.poll() is None for session in self.sessions.interact.values())
                or any(self.jobs.status(path.parent.name)['state'] not in TERMINAL for path in (self.root/'jobs').glob('*/status.json')))

    def dispatch_inner(self, method, params):
        if method in self.handlers:
            return self.handlers[method](method, params)
        if method == "ping":
            return {"protocol": PROTOCOL, "identity":self.identity.value, "daemon_id":self.identity.daemon_id,
                    "daemon_pid": os.getpid(), "state_dir": str(self.root)}
        if method == "doctor":
            from worker_native_broker import doctor as native_broker_doctor
            return {**doctor(self.root,self.trusted,self.isolated),'identity':self.identity.value,'daemon_id':self.identity.daemon_id,
                    'native_broker':native_broker_doctor(self.identity.value) if os.name=='nt' and not self.trusted else {'available':False,'reason':'not an installed metadata worker'},
                    'exclusive_run_lease':os.name=='nt' and not self.trusted}
        if method == "ghidra.query":
            from ghidra_backend import query
            return query(self.root, params)
        if method == "worker.shutdown":
            active = [self.jobs.status(path.parent.name) for path in (self.root / "jobs").glob("*/status.json")]
            if any(job["state"] not in TERMINAL for job in active):
                raise ValueError("cannot shut down worker while jobs are active; cancel and reconcile them first")
            self.sessions.close_all()
            return {"state": "shutdown"}
        raise ValueError("unsupported worker method: " + method)
