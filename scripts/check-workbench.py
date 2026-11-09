#!/usr/bin/env python3
"""Live Docker acceptance; only disposable resources with this invocation's ID."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import time
import uuid


def main():
    parser=argparse.ArgumentParser();parser.add_argument("--docker",default="docker");parser.add_argument("--image",default="re-agent-workbench:0.1");args=parser.parse_args()
    prefix="re-agent-check-"+uuid.uuid4().hex[:10]
    containers=[];networks=[];volumes=[]
    def docker(*argv):
        result=subprocess.run([args.docker,*map(str,argv)],capture_output=True,text=True,timeout=180)
        if result.returncode:raise RuntimeError("Docker "+str(argv[:2])+": "+result.stderr[-4000:])
        return result.stdout.strip()
    def network(suffix,internal=True):
        name=prefix+"-"+suffix;docker("network","create",*(["--internal"] if internal else []),"--label","re-agent.acceptance="+prefix,name);networks.append(name);return name
    def run(suffix,*argv):
        name=prefix+"-"+suffix;containers.append(name);docker("run","-d","--name",name,"--label","re-agent.acceptance="+prefix,*argv);return name
    def volume(suffix):
        name=prefix+"-"+suffix;docker("volume","create","--label","re-agent.acceptance="+prefix,name);volumes.append(name);return name
    def request(container,expected):
        code="import socket; s=socket.create_connection(('challenge',1337),5); assert s.recv(10)=="+repr(expected)
        for attempt in range(20):
            try:docker("exec",container,"python3","-c",code);return
            except RuntimeError:
                if attempt==19:raise
                time.sleep(.1)
    try:
        tool_names=[]
        for number in [1,2]:
            net=network(str(number));marker=str(number).encode()
            server="import socket; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('0.0.0.0',1337)); s.listen();\nwhile True:\n c,a=s.accept(); c.sendall("+repr(marker)+"); c.close()"
            run("service"+str(number),"--network",net,"--network-alias","challenge","--cap-drop","ALL","--read-only","--entrypoint","python3",args.image,"-u","-c",server)
            tool=run("tool"+str(number),"--network",net,"--cap-drop","ALL","--read-only",args.image);tool_names.append(tool);request(tool,marker)
            state=json.loads(docker("inspect",tool))[0]
            assert all(m["Type"]=="volume" and m["Destination"]=="/state" for m in state["Mounts"]) and not state["HostConfig"]["PortBindings"]
        print("PASS: two sibling service/tool pairs resolve challenge independently")
        private=network("relay-private");egress=network("relay-egress",False)
        server="import socket; s=socket.socket(); s.bind(('0.0.0.0',1337)); s.listen();\nwhile True:\n c,a=s.accept(); c.sendall(b'remote'); c.close()"
        run("upstream","--network",egress,"--network-alias","upstream","--entrypoint","python3",args.image,"-u","-c",server)
        relay=run("relay","--network",egress,"--read-only","--cap-drop","ALL","--user","65534:65534","--entrypoint","python3",args.image,"/opt/re-agent/runtime/workbench_relay.py","upstream","1337")
        docker("network","connect","--alias","challenge",private,relay)
        tool=run("relay-tool","--network",private,"--read-only","--cap-drop","ALL",args.image);request(tool,b"remote")
        assert list(json.loads(docker("inspect",tool))[0]["NetworkSettings"]["Networks"])==[private]
        print("PASS: fixed upstream relay, tool connected only to internal network")
        source=volume("source");destination=volume("restored")
        with tempfile.TemporaryDirectory(prefix=prefix) as directory:
            docker("run","--rm","--network","none","--mount",f"type=volume,source={source},target=/snapshot","--entrypoint","python3",args.image,"-c","from pathlib import Path; p=Path('/snapshot'); (p/'files').mkdir(); (p/'files/input').write_bytes(b'payload'); (p/'debug/id').mkdir(parents=True); (p/'debug/id/session.json').write_text('{\"state\":\"running\",\"gdb_pid\":123}'); (p/'endpoint.json').write_text('{\"auth\":\"old-secret\"}')")
            for action,vol in [("export",source),("restore",destination)]:
                docker("run","--rm","--network","none","--read-only","--mount",f"type=volume,source={vol},target=/snapshot","--mount",f"type=bind,source={directory},target=/bundle","--entrypoint","python3",args.image,"/opt/re-agent/runtime/workbench_archive.py",action,"/snapshot","/bundle/state.tar")
            docker("run","--rm","--network","none","--mount",f"type=volume,source={destination},target=/snapshot,readonly","--entrypoint","python3",args.image,"-c","from pathlib import Path; import json; p=Path('/snapshot'); assert (p/'files/input').read_bytes()==b'payload'; s=json.loads((p/'debug/id/session.json').read_text()); assert s['state']=='lost' and 'gdb_pid' not in s; assert not (p/'endpoint.json').exists()")
        print("PASS: volume round trip preserves data and discards process/auth authority")
        flags=["--cap-drop","ALL"]
        for cap in ["CHOWN","SETUID","SETGID","SYS_PTRACE","DAC_OVERRIDE"]:flags.extend(["--cap-add",cap])
        output=docker("run","--rm","--network","none","--read-only","--tmpfs","/tmp:rw,size=1g,exec",*flags,"--security-opt","no-new-privileges","--security-opt","seccomp=unconfined","--entrypoint","python3",args.image,"-m","unittest","discover","-s","/opt/re-agent/runtime","-p","test_workbench*.py","-v")
        print("PASS: shared launch runtime and archive functional tests")
    finally:
        for name in reversed(containers):
            subprocess.run([args.docker,"rm","-f","-v",name],capture_output=True)
        for name in reversed(networks):subprocess.run([args.docker,"network","rm",name],capture_output=True)
        for name in reversed(volumes):subprocess.run([args.docker,"volume","rm",name],capture_output=True)

if __name__=="__main__":main()
