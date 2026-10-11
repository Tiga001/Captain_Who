from pathlib import Path
import subprocess,json,hashlib
b=Path(__file__).resolve().parent
binary=Path('/tmp/captain-backend-baseline-20261011-target/release/deps/core_server-f7a97b10b9960ccf')
name='application::agent::turn_executor::runtime_isolation_tests::receipt_recovery_wait_does_not_occupy_completion_worker'
results=[]
with (b/'recovery-barrier-repeat.log').open('w') as f:
 for i in range(20):
  r=subprocess.run([str(binary),name,'--exact','--test-threads=1','--nocapture'],stdout=f,stderr=subprocess.STDOUT)
  results.append({'iteration':i+1,'exit_code':r.returncode})
  if r.returncode:break
receipt={'binary':str(binary),'sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'test':name,'iterations':results,'purpose':'Deterministic worker gate replaces an obsolete global-lock timing assumption; repeats follow the single final regression suite and are not added to unique test counts.'}
(b/'recovery-barrier-repeat.json').write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps({'iterations':len(results),'failures':sum(bool(r['exit_code']) for r in results)}))
raise SystemExit(1 if any(r['exit_code'] for r in results) or len(results)!=20 else 0)
