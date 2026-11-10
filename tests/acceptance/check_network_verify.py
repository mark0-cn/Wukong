#!/usr/bin/env python3
"""Exercise the real replay engine between two disposable Docker containers.

The image needs Python 3 only. Runtime source is mounted read-only; no Docker
socket, application state or credentials enter either container. This checks
transport/replay, not installation of the full workbench toolchain.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid

SERVER = r'''
import secrets, socket
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('0.0.0.0', 1337)); s.listen()
print('ready', flush=True)
while True:
    c, _ = s.accept()
    with c:
        c.settimeout(5)
        nonce = secrets.randbits(48)
        c.sendall(b'nonce:%016x\n> ' % nonce)
        data = bytearray()
        while len(data) < 8:
            chunk = c.recv(8-len(data))
            if not chunk: break
            data.extend(chunk)
        c.sendall(b'accepted:marker\n' if len(data)==8 and int.from_bytes(data,'little')==nonce+1 else b'rejected\n')
'''


def client(output):
    from worker_network import network_profile, replay_network
    root = Path('/tmp/replay'); root.mkdir()
    base = root/'files/challenge'; base.mkdir(parents=True)
    target = base/'target'; target.write_bytes(b'trusted protocol identity fixture; never executed')
    launch = {'schema_version':1, 'challenge_id':'sibling-replay', 'base':'files/challenge',
              'target':'target', 'files':{'target':hashlib.sha256(target.read_bytes()).hexdigest()},
              'service':{'mode':'local_service', 'port':1337}}
    (base/'launch.json').write_text(json.dumps(launch))
    encode = lambda data: base64.b64encode(data).decode()
    plan = {'steps':[
        {'op':'receive','until_base64':encode(b'> '),'max_bytes':64},
        {'op':'capture','name':'nonce','offset':6,'length':16,'encoding':'hex'},
        {'op':'send','offset':0,'length':8,'patches':[
            {'offset':0,'capture':'nonce','addend':1,'width':8,'endianness':'little'}]},
        {'op':'receive','until_base64':encode(b'\n'),'max_bytes':64},
        {'op':'expect','contains_base64':encode(b'accepted:marker')}]}
    spec = {'target':'files/challenge/target','launch_spec':'files/challenge/launch.json',
            'network':plan,'timeout_secs':5}
    spec['network_binding'] = network_profile(root,spec)
    assert spec['network_binding']['transport_host']=='challenge'
    trials=root/'trials'; trials.mkdir()
    observations=[]
    for index in range(3):
        if index==2: plan['steps'][2]['patches'][0]['addend']=2
        result=replay_network(root,trials,spec,b'\0'*8,index)
        assert result['network_success']==(index<2), result
        assert result['fresh_connection'] and not result['server_reset']
        assert not result['network_binding']['runtime_identity_confirmed']
        raw=(root/result['transcript_path']).read_bytes()
        assert hashlib.sha256(raw).hexdigest()==result['transcript_sha256']
        (output/('transcript-%d.json' % index)).write_bytes(raw)
        observations.append(result)
    # A modified prepared service must fail before a new connection is opened.
    launch['service']['port']=1338
    (base/'launch.json').write_text(json.dumps(launch))
    try: replay_network(root,trials,spec,b'\0'*8,3)
    except ValueError as error: assert 'identity changed' in str(error)
    else: raise AssertionError('changed service binding was accepted')
    (output/'results.json').write_text(json.dumps({'passed':True,'observations':observations},indent=2)+'\n')
    print('PASS: sibling TCP replay, fresh capture/patch, negative response, changed binding and transcript hashes')


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', help='existing image containing python3')
    parser.add_argument('--docker', default='docker')
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--client', action='store_true', help=argparse.SUPPRESS)
    args=parser.parse_args()
    if args.client:
        client(args.output_dir)
        return
    if not args.image: parser.error('--image is required')
    prefix='wukong-network-check-'+uuid.uuid4().hex[:10]
    output=args.output_dir.resolve()/prefix; output.mkdir(parents=True)
    root=Path(__file__).resolve().parents[2]
    containers=[]
    created_network=False

    def docker(*argv):
        result=subprocess.run([args.docker,*map(str,argv)],capture_output=True,text=True,timeout=120)
        if result.returncode: raise RuntimeError(result.stderr[-4000:])
        return result.stdout.strip()

    try:
        docker('network','create','--internal','--label','wukong.acceptance='+prefix,prefix)
        created_network=True
        service=prefix+'-service';containers.append(service)
        # With all capabilities dropped, container root cannot bypass a Linux
        # runner's directory permissions. Keep evidence owned by the caller.
        user=f'{os.getuid()}:{os.getgid()}' if os.name=='posix' else '1000:1000'
        common=['--user',user,'--network',prefix,'--cap-drop','ALL','--read-only','--security-opt','no-new-privileges',
                '--label','wukong.acceptance='+prefix]
        docker('run','-d','--name',service,*common,'--network-alias','challenge',
               '--entrypoint','python3',args.image,'-u','-c',SERVER)
        for attempt in range(50):
            if 'ready' in docker('logs',service): break
            if attempt==49: raise RuntimeError('fixture server did not become ready')
            time.sleep(.1)
        tool=prefix+'-tool';containers.append(tool)
        print(docker('run','--name',tool,*common,'--tmpfs','/tmp:rw,size=32m',
                     '--mount',f'type=bind,source={root / "workers/runtime"},target=/runtime,readonly',
                     '--mount',f'type=bind,source={Path(__file__).resolve()},target=/check.py,readonly',
                     '--mount',f'type=bind,source={output},target=/evidence',
                     '--env','PYTHONPATH=/runtime','--env','PYTHONDONTWRITEBYTECODE=1',
                     '--entrypoint','python3',args.image,'/check.py','--client','--output-dir','/evidence'))
        state=json.loads(docker('inspect',tool))[0]
        assert list(state['NetworkSettings']['Networks'])==[prefix]
        assert not state['HostConfig']['PortBindings']
        assert {m['Destination'] for m in state['Mounts']}=={'/runtime','/check.py','/evidence'}
        (output/'container.json').write_text(json.dumps({'image_id':state['Image'],'network':prefix,
            'runtime_hashes':{p.name:hashlib.sha256(p.read_bytes()).hexdigest()
                              for p in (root/'workers/runtime').glob('worker_*.py')}},indent=2)+'\n')
        print('Evidence: '+str(output))
    finally:
        for name in reversed(containers):
            subprocess.run([args.docker,'rm','-f',name],capture_output=True)
        if created_network:
            subprocess.run([args.docker,'network','rm',prefix],capture_output=True)


if __name__=='__main__': main()
