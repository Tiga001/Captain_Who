import json,gzip,pathlib,collections,subprocess
base=pathlib.Path(__file__).resolve().parent
root=next(p for p in base.parents if (p/'Cargo.toml').is_file())
matrix=root/'.cache/backend-concurrency/round5-full-20261011-r1'
comparison=json.loads((base/'comparison-final.json').read_text())
metadata=json.loads((matrix/'metadata.json').read_text())
regressions=json.loads((base/'regressions/accepted-regression-summary.json').read_text())
def db_metrics(path):
    values=collections.Counter()
    with (gzip.open(path,'rt') if path.suffix=='.gz' else path.open()) as f:
        for line in f:
            for stat in json.loads(line)['stage_metrics']:
                name=stat['stage']+'/'+stat['operation']
                if stat['stage'] in ('sqlite.wait','sqlite.read_wait','sqlite.hold','sqlite.read_hold','storage.queue_wait','storage.worker_wait'):
                    values[stat['stage']+'.seconds']+=stat['total_us']/1e6
                if name=='storage.queue_wait/writer':
                    values['writer_reservation_estimated_bytes']+=stat.get('bytes',0)
                    values['writer_reservation_count']+=stat['count']
    values['instrumented_db_wait_seconds']=sum(values[k+'.seconds'] for k in ('sqlite.wait','sqlite.read_wait','storage.queue_wait','storage.worker_wait'))
    values['connection_occupancy_seconds']=sum(values[k+'.seconds'] for k in ('sqlite.hold','sqlite.read_hold'))
    return dict(values)
result={
 'round':5,'status':'implementation_and_round_validation_complete',
 'start_commit':'df3e6f04ee1a8f14c8447b34841c76e50409ccbf',
 'implementation_commit':metadata['source']['head'],
 'run_id':matrix.name,'run_status':metadata['status'],
 'source_changed_during_run':metadata['source_changed_during_run'],
 'source':{k:metadata['source'][k] for k in ('head','source_manifest_sha256','tracked_diff_sha256')},
 'benchmark_binary':metadata['binary'],'machine':metadata['machine'],'run':metadata['run'],
 'results':metadata['results'],'comparison_acceptance':comparison['acceptance'],
 'comparison_violations':comparison['comparison_violations'],
 'outcome_changes_requiring_interpretation':comparison['outcome_changes_requiring_interpretation'],
 'correctness':{k:v for k,v in regressions.items() if k not in ('test_names','suites')},
 'host_receipt':json.loads((base/'host-read-receipt.json').read_text()),
 'boundary':comparison['boundary'],
 'limitations':['Round six soak, disk-full and full native UI end-to-end gates remain.',
 'One full matrix per revision is not statistical significance or maximum throughput.',
 'Core adds necessary headless persistence absent in the earlier no-Renderer baseline.',
 'Writer reservation bytes are estimated logical payload budgets, not physical SQLite/disk bytes.',
 '300ms checkpoint scheduling is not a hard bound under writer congestion or failure.',
 'Done retains authoritative full-history read; selected synchronous barriers remain.',
 '50-way send p95, steady p95, CPU, DB waits and estimated writer payload increased; no all-stack performance pass is claimed.'],
 'before':{k:v for k,v in comparison['before'].items() if k!='cases'},
 'after':{k:v for k,v in comparison['after'].items() if k!='cases'},
}
result['before']['database_aggregates']=db_metrics(root/'docs/performance/backend-concurrency/results/2026-10-11-round4/cases.jsonl.gz')
result['after']['database_aggregates']=db_metrics(matrix/'cases.jsonl')
(matrix/'summary.json').write_text(json.dumps(result,indent=2,ensure_ascii=False)+'\n')
print(json.dumps(result['correctness'],indent=2))
print(json.dumps({k:result[k]['database_aggregates'] for k in ('before','after')},indent=2))
