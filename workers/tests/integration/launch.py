"""Execute only this embedded development fixture in configured Linux isolation."""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
from support.client import Client

SOURCE=r'''
#include <stdio.h>
#include <string.h>
int main(int argc,char **argv) {
    if(argc<3) return 2;
    FILE *peer=fopen(argv[2],"rb");if(!peer) return 3;
    char marker[12];size_t count=fread(marker,1,sizeof(marker),peer);fclose(peer);
    if(count!=12 || memcmp(marker,"wukong-peer\n",12)) return 4;
    FILE *output=fopen("created.bin","wb");if(!output) return 5;
    if(fwrite(marker,1,count,output)!=count) return 6;fclose(output);
    FILE *input=strcmp(argv[1],"stdin")==0?stdin:(argc==4?fopen(argv[3],"rb"):NULL);
    if(!input) return 7;
    unsigned char bytes[4096];
    while((count=fread(bytes,1,sizeof(bytes),input))!=0) if(fwrite(bytes,1,count,stdout)!=count) return 8;
    if(input!=stdin) fclose(input);
    return 0;
}
'''

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--state',type=Path,required=True);args=parser.parse_args()
    state=args.state.resolve();state.mkdir(parents=True,exist_ok=False);state.chmod(0o700)
    with tempfile.TemporaryDirectory() as temporary:
        root=Path(temporary);(root/'fixture.c').write_text(SOURCE)
        subprocess.run(['gcc','-O0','-g',str(root/'fixture.c'),'-o',str(root/'fixture')],check=True)
        binary=(root/'fixture').read_bytes()
    client=Client(state)
    try:
        doctor=client.call('doctor');assert doctor['capabilities']['process_jobs'],doctor
        program=client.put('case/bin/target',binary,True)
        peer=b'wukong-peer\n';client.put('case/data/peer.bin',peer)
        data=b'launch\x00\xff\r\n';payload=client.put('inputs/data.bin',data)
        (state/'files/case/work').mkdir()
        reports=[]
        for mode in ['stdin_pipe','argv_file']:
            spec=dict(schema_version=1,base='files/case',target='bin/target',files={'bin/target':hashlib.sha256(binary).hexdigest(),'data/peer.bin':hashlib.sha256(peer).hexdigest()},args=['stdin' if mode=='stdin_pipe' else 'argv','../data/peer.bin']+(['@@'] if mode=='argv_file' else []),cwd='work',input_mode=mode,env={},timeout_secs=5,loader=None,library_path=None)
            manifest=client.put('case/'+mode+'.json',json.dumps(spec).encode())
            params=dict(launch_spec=manifest['path'],input=payload['path'],request_id='launch-'+mode)
            job=client.call('job.start',**params);status=client.finish(job)
            assert status['state']=='completed' and status['result']['exit_code']==0,status
            assert status['result']['metadata_protected'] and status['result']['experiment_uid']>0,status
            stdout=client.call('file.get',path='jobs/'+job['job_id']+'/output.stdout')
            assert base64.b64decode(stdout['data_base64'])==data,stdout
            artifacts=client.call('job.artifacts',job_id=job['job_id'])
            marker=next(item for item in artifacts if item['path'].endswith('/created.bin'))
            observed=client.call('file.get',path=marker['path'])
            assert base64.b64decode(observed['data_base64'])==peer,observed
            assert client.call('job.start',**params)['job_id']==job['job_id']
            reports.append(dict(mode=mode,status=status,stdout=stdout,created=observed))
        assert hashlib.sha256((state/program['path']).read_bytes()).hexdigest()==program['sha256']
        assert not (state/'files/case/work/created.bin').exists()
        output=dict(passed=True,scope='linux_isolated_launch_spec',source_sha256=hashlib.sha256(SOURCE.encode()).hexdigest(),cases=reports)
        (state/'launch-results.json').write_text(json.dumps(output,indent=2))
        print(json.dumps(dict(passed=True,scope=output['scope'],cases=len(reports),report=str(state/'launch-results.json'))))
    finally:client.close()

if __name__=='__main__':main()
