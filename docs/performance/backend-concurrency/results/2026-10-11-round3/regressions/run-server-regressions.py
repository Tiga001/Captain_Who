import json, pathlib, re, subprocess, time, sys
base = pathlib.Path('.cache/backend-concurrency/round3-validation-20261011/accepted-server-regressions')
base.mkdir(exist_ok=True)
target = pathlib.Path('/tmp/captain-backend-baseline-20261011-target/release/deps')
suites = []
previous=pathlib.Path('docs/performance/backend-concurrency/results/2026-10-11-round2/regressions/run-regressions.py').read_text()
import ast
module=ast.parse(previous)
for node in module.body:
    if isinstance(node,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='suites' for t in node.targets):
        suites += [v for v in ast.literal_eval(node.value) if v[0]=='server']
suites += [('server','application::agent::tests::automation_turn'),('server','provider_transition'),('server','application::agent::tests::provider_profiles'),('server','application::agent::tests::human_root_notifications'),('server','application::agent::tests::pending_actions'),('server','application::agent_wait::tests')]
start_index=int(sys.argv[1]) if len(sys.argv)>1 else 0
results=json.loads((base/'regressions.json').read_text())[:start_index] if start_index and (base/'regressions.json').exists() else []
for i,(kind,pattern) in enumerate(suites):
    if i<start_index: continue
    binary=target/('mycopilot_core-7748a31bd053ad1f' if kind=='core' else 'core_server-f7a97b10b9960ccf')
    path=base/('final-%02d-%s.log'%(i,pattern.replace('::','-')))
    start=time.monotonic()
    with path.open('w') as out:
        p=subprocess.run([str(binary),pattern,'--test-threads=4' if kind=='core' else '--test-threads=1','--nocapture'],stdout=out,stderr=subprocess.STDOUT,timeout=900)
    output=path.read_text()
    matched=re.search(r'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored',output)
    r=dict(kind=kind,filter=pattern,returncode=p.returncode,duration_s=round(time.monotonic()-start,3),log=path.name,counts=matched.groups() if matched else None)
    results.append(r);(base/'regressions.json').write_text(json.dumps(results,indent=2)+'\n');print(json.dumps(r),flush=True)
    if p.returncode or not matched or int(matched.group(2))==0: sys.exit(1)
