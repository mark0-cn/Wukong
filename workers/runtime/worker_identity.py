"""Installation/boot/content identity for Linux workers and trusted development fixtures."""
import functools
import hashlib
import json
import os
from pathlib import Path
import re
import uuid

from worker_common import PROTOCOL, WorkerError, load_json


@functools.lru_cache(maxsize=1)
def boot_identity():
    if os.name != "nt":
        return "linux:" + Path('/proc/sys/kernel/random/boot_id').read_text().strip()
    from worker_windows_identity import windows_boot_identity
    return windows_boot_identity()


def installation_identity(root):
    path = Path(root) / 'installation.json'
    if not path.exists():
        try:
            with path.open('x',encoding='utf-8') as stream:
                os.chmod(path,0o600)
                json.dump({'protocol':PROTOCOL,'installation_id':uuid.uuid4().hex},stream)
                stream.flush()
                os.fsync(stream.fileno())
        except FileExistsError:
            pass
    document=load_json(path)
    if document.get('protocol') != PROTOCOL:
        raise ValueError('unsupported worker state protocol; use a new worker state directory')
    value=document['installation_id']
    if not isinstance(value,str) or not re.fullmatch(r'[0-9a-f]{32}',value):
        raise ValueError('invalid worker installation identity; never silently replace it')
    return value


def content_identity(root):
    digest=hashlib.sha256()
    for path in sorted(Path(__file__).parent.glob('*.py')):
        digest.update(path.name.encode()+b'\0'+path.read_bytes()+b'\0')
    schema = Path(__file__).resolve().parent.parent / 'protocol/schema/contract.json'
    digest.update(b'protocol/schema/contract.json\0' + schema.read_bytes() + b'\0')
    return digest.hexdigest()


class WorkerIdentity:
    def __init__(self,root):
        self.value={'protocol':PROTOCOL,'installation_id':installation_identity(root),
                    'boot_id':boot_identity(),'package_sha256':content_identity(root)}
        self.daemon_id=uuid.uuid4().hex

    def validate(self,binding):
        if binding != self.value:
            raise WorkerError('binding_changed','worker installation, boot, package or protocol binding changed')
