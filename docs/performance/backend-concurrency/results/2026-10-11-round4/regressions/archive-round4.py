from pathlib import Path
import gzip,hashlib,json,shutil,sys
validation=Path(__file__).resolve().parent
repo=next(p for p in validation.parents if (p/'Cargo.toml').is_file())
run=Path(sys.argv[1]).resolve()
dest=repo/'docs/performance/backend-concurrency/results/2026-10-11-round4'
dest.mkdir(parents=True,exist_ok=False)
def copy(source,target,compress=False):
    target.parent.mkdir(parents=True,exist_ok=True)
    if compress:
        target.write_bytes(gzip.compress(source.read_bytes(),mtime=0))
    else:shutil.copy2(source,target)
for name in ['metadata.json','resources.jsonl']:
    copy(run/name,dest/name)
for name in ['cases.jsonl','test.stdout.log','test.stderr.log','build.stdout.log','build.stderr.log']:
    copy(run/name,dest/(name+'.gz'),True)
for suffix in ['json','csv','txt']:
    source=validation/('comparison-final.'+suffix)
    target=dest/('comparison.'+suffix+('.gz' if suffix=='json' else ''))
    copy(source,target,suffix=='json')
copy(validation/'analyze-round4.py',dest/'compare-results.py')
for source in (validation/'regressions').iterdir():
    if source.suffix=='.log':copy(source,dest/'regressions'/(source.name+'.gz'),True)
    elif source.suffix=='.json':copy(source,dest/'regressions'/source.name)
for name in ['build-final.log','clippy-final.log','format-final.log','docs-format-final.log','docs-check-final.log','test-infrastructure.log','test-layout.log','comparison-script-validation.log']:
    copy(validation/name,dest/'regressions'/(name+'.gz'),True)
for name in ['run-regressions.py','summarize-regressions.py','archive-round4.py']:
    copy(validation/name,dest/'regressions'/name)
for name in ['build-initial.log','build-third.log','clippy-initial.log']:
    copy(validation/name,dest/'diagnostics'/(name+'.gz'),True)
for directory in ['early-regressions','pre-final-smoke','pre-final-regressions','sixth-regressions']:
    for source in (validation/directory).iterdir():
        if source.suffix=='.log' and ('FAILED' in source.read_text() or 'error:' in source.read_text()):
            copy(source,dest/'diagnostics'/directory/(source.name+'.gz'),True)
print(dest)
