"""Installation/boot/content binding and persistent exclusive Windows run fences."""
import functools
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import threading
import time
import uuid

from worker_common import PROTOCOL, WorkerError, atomic_json, load_json, safe_env


@functools.lru_cache(maxsize=1)
def boot_identity():
    if os.name != "nt":
        return "linux:" + Path('/proc/sys/kernel/random/boot_id').read_text().strip()
    executable = Path(os.environ['SystemRoot']) / 'System32/WindowsPowerShell/v1.0/powershell.exe'
    result = subprocess.run([str(executable),'-NoLogo','-NoProfile','-NonInteractive','-Command',
        '[Console]::Write((Get-CimInstance -ClassName Win32_OperatingSystem).LastBootUpTime.ToUniversalTime().Ticks)'],
        env=safe_env(),capture_output=True,timeout=15,creationflags=subprocess.CREATE_NO_WINDOW)
    value=result.stdout.decode('ascii').strip()
    if result.returncode or not re.fullmatch(r'[0-9]{16,20}',value):
        raise ValueError('cannot establish the Windows boot identity')
    return 'windows:' + value


def installation_identity(root):
    path = Path(root) / 'installation.json'
    if not path.exists():
        try:
            with path.open('x',encoding='utf-8') as stream:
                os.chmod(path,0o600)
                json.dump({'installation_id':uuid.uuid4().hex},stream)
                stream.flush()
                os.fsync(stream.fileno())
        except FileExistsError:
            pass
    value=load_json(path)['installation_id']
    if not isinstance(value,str) or not re.fullmatch(r'[0-9a-f]{32}',value):
        raise ValueError('invalid worker installation identity; never silently replace it')
    return value


def content_identity(root):
    digest=hashlib.sha256()
    for path in sorted(Path(__file__).parent.glob('*.py')):
        digest.update(path.name.encode()+b'\0'+path.read_bytes()+b'\0')
    backend=Path(root)/'windows.backend.json'
    if backend.is_file():
        profile=load_json(backend)
        for key in ['helper_sha256','engine_sha256']:
            digest.update(str(profile[key]).encode()+b'\0')
    return digest.hexdigest()


class WorkerIdentity:
    def __init__(self,root):
        self.value={'protocol':PROTOCOL,'installation_id':installation_identity(root),
                    'boot_id':boot_identity(),'package_sha256':content_identity(root)}
        binding_path=os.environ.get('WUKONG_NATIVE_BINDING_FILE')
        if binding_path:
            if os.name!='nt' or Path(binding_path).resolve()!=Path(root).resolve().parent/'binding.json':
                raise ValueError('native installation binding must come from the fixed protected parent')
            self.value['package_sha256']=native_package_identity(load_json(binding_path),self.value['installation_id'])
        self.daemon_id=uuid.uuid4().hex

    def validate(self,binding):
        if binding != self.value:
            raise WorkerError('binding_changed','worker installation, boot, package or protocol binding changed')


def native_package_identity(binding,installation):
    """The native SCM service verifies the signed tree before launch. Its binding
    file is System/admin-owned and read-only to the ordinary worker account.
    This binds an installed epoch to the whole sealed payload, including native
    broker/engine contents; it does not enable production execution capabilities.
    """
    if (not isinstance(binding,dict) or set(binding)!={'schema','protocol','installation_id','package_sha256'}
            or binding['schema']!=1 or binding['protocol']!=PROTOCOL or binding['installation_id']!=installation
            or not isinstance(binding['package_sha256'],str) or not re.fullmatch(r'[0-9a-f]{64}',binding['package_sha256'])):
        raise ValueError('native installation binding differs from the worker installation')
    return binding['package_sha256']


class ExclusiveLease:
    """Serializes admission and persists epochs before any new owner can execute.

    An expired lease does not prove its processes died. A different run can only
    acquire after explicit cleanup establishes that all owned resources are idle.
    """
    def __init__(self,root,identity,active,clock=time.monotonic):
        self.path=Path(root)/'exclusive-lease.json'
        self.identity=identity
        self.active=active
        self.clock=clock
        self.lock=threading.RLock()
        self.inflight=0

    def _load(self):
        value=load_json(self.path) if self.path.exists() else {'epoch':0}
        if value.get('boot_id') != self.identity.value['boot_id']:
            return {'epoch':value['epoch']}
        return value

    def acquire(self,params):
        with self.lock:
            run,owner=params.get('run_id'),params.get('owner_id')
            if any(not isinstance(v,str) or not v or len(v)>256 for v in [run,owner]):
                raise ValueError('lease requires bounded run and controller identities')
            now=self.clock();value=self._load()
            same=value.get('run_id')==run and value.get('owner_id')==owner
            if value.get('expires_at',0)>now and not same:
                raise WorkerError('conflict','another controller holds the Windows environment lease')
            if value.get('expires_at',0)<=now and self.inflight:
                raise WorkerError('conflict','a previous request is still executing; its outcome must be reconciled')
            if value.get('run_id')!=run and self.active():
                raise WorkerError('conflict','owned resources remain; reconcile them before assigning another run')
            epoch=value['epoch']+(0 if same and value.get('expires_at',0)>now else 1)
            value={'epoch':epoch,'run_id':run,'owner_id':owner,'boot_id':self.identity.value['boot_id'],
                   'expires_at':now+120,'token':value['token'] if same and value.get('expires_at',0)>now else uuid.uuid4().hex}
            atomic_json(self.path,value)
            return value

    def validate(self,token):
        value=self._load()
        if (not isinstance(token,dict) or value.get('expires_at',0)<=self.clock()
                or any(value.get(key)!=token.get(key) for key in ['epoch','run_id','owner_id','token','boot_id'])):
            raise WorkerError('lease_lost','Windows environment lease expired or was fenced by a successor')
        return value

    def release(self,params):
        with self.lock:
            value=self.validate(params.get('lease'))
            if self.active():
                raise WorkerError('conflict','close sessions and reconcile active jobs before releasing the environment')
            atomic_json(self.path,{'epoch':value['epoch'],'boot_id':value['boot_id']})
            return {'released':True,'epoch':value['epoch']}

    def dispatch(self,params,operation):
        with self.lock:
            self.identity.validate(params.get('_binding'))
            self.validate(params.get('_lease'))
            self.inflight+=1
        try:
            # Leases are validated before looking at durable receipts; their
            # changing expiry/epoch must not alter the business request hash.
            return operation({key:value for key,value in params.items() if key not in {'_binding','_lease'}})
        finally:
            with self.lock:
                self.inflight-=1
