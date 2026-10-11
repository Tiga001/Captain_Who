"""Deterministic raw-sample analysis; successes, errors and unfinished stay separate."""
import json, gzip, math, random, pathlib, sys, subprocess, collections, copy

def read_cases(path):
    path=pathlib.Path(path)
    opener=gzip.open if path.suffix=='.gz' else open
    with opener(path,'rt') as f:return [json.loads(line) for line in f if line.strip()]

def quantiles(values):
    values=sorted(values)
    return {'n':len(values),**{key:(values[max(0,math.ceil(len(values)*q)-1)]/1000 if values else None) for key,q in [('p50_ms',.5),('p95_ms',.95),('p99_ms',.99),('max_ms',1)]}}

def block_ci(records):
    # Resample whole 10-second blocks; never treat correlated roots in one burst as independent.
    blocks={}
    for r in records:blocks.setdefault(r['scheduled_us']//10_000_000,[]).append(r['responded_us']-r['scheduled_us'])
    blocks=list(blocks.values())
    if len(blocks)<20:return {'status':'insufficient_blocks','blocks':len(blocks)}
    rng=random.Random(61011); ps={.95:[],.99:[]}
    for _ in range(400):
        values=sorted(v for b in rng.choices(blocks,k=len(blocks)) for v in b)
        for p in ps:ps[p].append(values[math.ceil(len(values)*p)-1]/1000)
    return {'status':'within_run_block_bootstrap','blocks':len(blocks),'replicates':400,
        **{f'p{int(p*100)}_95pct_ci_ms':[sorted(v)[9],sorted(v)[389]] for p,v in ps.items()},
        'limit':'Conditional on this synthetic run; not independent process/machine or real-provider confidence.'}

def transport_healthy(x):
    raw=x.get('raw',{}); outbound=x.get('outbound',{})
    has_failure='transport_failure' in raw; has_state='state' in outbound
    # Legacy archives have neither field. Partial new evidence is not healthy evidence.
    if not has_failure and not has_state:return True
    state=outbound.get('state')
    return (has_failure and raw['transport_failure'] is None and has_state
        and isinstance(state,dict)
        and all(state.get(key) is False for key in ['closed','failed','overloaded'])
        and 'failure_snapshot' in state and state['failure_snapshot'] is None)

def analyze(x):
    records=list(x['raw']['rpcs'].values()); ops={}
    for op in sorted({r['operation'] for r in records}):
        rows=[r for r in records if r['operation']==op]
        good=[r for r in rows if r.get('response') is not None and 'result' in r['response']]
        bad=[r for r in rows if r.get('response') is not None and 'error' in r['response']]
        all_responses=[r for r in rows if r.get('responded_us') is not None]
        ops[op]={'offered':len(rows),'dispatch_admitted':sum(r['dispatch_admitted'] for r in rows),'handler_started':sum(r['handler_started_us'] is not None for r in rows),'transport_rejected':sum(r.get('transport_rejected_us') is not None for r in rows),'success':len(good),'errors':len(bad),'no_response':len(rows)-len(all_responses),
            'success_scheduled':quantiles([r['responded_us']-r['scheduled_us'] for r in good]),
            'all_scheduled':quantiles([r['responded_us']-r['scheduled_us'] for r in all_responses]),
            'success_actual':quantiles([r['responded_us']-r['offered_us'] for r in good]),
            'queue':quantiles([r['handler_started_us']-r['offered_us'] for r in rows if r['handler_started_us'] is not None]),
            'provider_start':quantiles([x['raw']['provider_first_us'][r['provider_key']]-r['scheduled_us'] for r in rows if r.get('provider_key') in x['raw']['provider_first_us']]),
            'response_bytes':sum(r.get('response_bytes',0) for r in rows),'confidence':block_ci(good)}
    peaks={}
    for q in [q for sample in x['raw'].get('resource_samples',[]) for q in sample['queues']]+x['queue_metrics']:
        key=q['stage']+'/'+q['operation']; out=peaks.setdefault(key,{'peak_count':0,'peak_bytes':0,'sampled_oldest_max_us':0,'omitted':0})
        for field in ['peak_count','peak_bytes']:out[field]=max(out[field],q[field])
        out['sampled_oldest_max_us']=max(out['sampled_oldest_max_us'],q['oldest_us']);out['omitted']+=q['omitted']
    before=x['resources_before'];after=x['resources_after'];cpu=sum(after[k]-before[k] for k in ['cpu_user_seconds','cpu_system_seconds'])
    diskio={k:after['process_io'][k]-before['process_io'][k] for k in ['diskio_bytes_read','diskio_bytes_written','logical_writes','pageins']} if before.get('process_io') and after.get('process_io') else None
    roots=x['recovery']['root_terminal_statuses']; starts=ops['agent.startConversationTurn']; interval=x['fixed_wave_interval_ms']; seconds=x['measured_us']/1e6
    checks={'no_rejections_or_errors':all(v['errors']==0 for v in ops.values()),'all_responded':all(v['no_response']==0 for v in ops.values()),'drained':x['drained_before_cleanup'],'recovered':x['recovered_root_turns']==x['root_starts_durably_accepted'] and x['recovery']['post_reopen_in_progress']==0,'send_p95_under_500ms':starts['success_scheduled']['p95_ms'] is not None and starts['success_scheduled']['p95_ms']<=500}
    for op in ['agent.approveAction','agent.cancelRun']:
        if op in ops:
            q=ops[op]['success_scheduled'];hi=x['case']['units']>20
            checks[op+'_empirical_latency']=q['p95_ms'] is not None and q['p95_ms']<=(500 if hi else 250) and q['p99_ms']<=(2000 if hi else 1000)
    owners={str(owner):quantiles([r['responded_us']-r['scheduled_us'] for r in records if r['owner']==owner and r['operation']=='agent.startConversationTurn' and (r.get('response') or {}).get('result')]) for owner in range(x['case']['roots'])}
    samples=x['raw'].get('resource_samples',[])
    result = {'case':x['name'],'configuration':x['case'],'operations':ops,'cpu_seconds':cpu,'process_disk_io_delta':diskio,'queue_peaks':peaks,'logical_active_sampled_peak':max((v['active_turns'] for v in samples),default=None),'sampled_wal_peak':max((v['disk']['wal_bytes'] for v in samples),default=None),'disk_after':x['disk_after'],'outbound':x['outbound'],'recovery':x['recovery'],'terminal_counts':roots,'offered_turns_per_second':x['case']['roots']*1000/interval,'measured_seconds':seconds,'completed_turns_per_second':None,'settled_turns_per_second':None,'sustainability_checks':checks,'empirical_slo_and_drain_pass':all(checks.values()),'slo_scope':'Core scheduled intake only; success quantiles never include rejections; small p99 remains exploratory.','owner_start_latency':owners,'retained_raw_json_bytes':len(json.dumps(x['raw'],separators=(',',':')).encode()),'rss_limit':'Process includes finite raw harness records and local fake provider; neither raw JSON bytes nor socket peak equals product heap.'}

    root = next(p for p in pathlib.Path(__file__).resolve().parents if (p/'Cargo.toml').is_file())
    validation = subprocess.run(['node', '--input-type=module', '-e',
        "import {validateBaselineCase} from './scripts/run-backend-concurrency-baseline.mjs';let s='';for await(const c of process.stdin)s+=c;console.log(JSON.stringify(validateBaselineCase(JSON.parse(s))))"],
        cwd=root, input=json.dumps(x), text=True, capture_output=True, check=True)
    errors = json.loads(validation.stdout)
    checks['raw_integrity'] = not errors
    checks['transport_healthy'] = transport_healthy(x)
    result['transport_failure'] = x['raw'].get('transport_failure')
    result['acknowledged_starts'] = x['root_starts_durably_accepted']
    result['durable_recovered_roots'] = x['recovered_root_turns']
    for key in ['starts_responded','controls_ready','replies_complete']:
        checks[key] = x.get(key) is True
    checks['runtime_error_free'] = not x['raw'].get('errors') and not x.get('unexpected_rpc_errors')
    checks['no_shutdown_timeout'] = x.get('shutdown_timed_out') is False
    checks['known_terminal_statuses'] = set(roots) <= {'Completed', 'Cancelled'}
    checks['no_unplanned_compaction'] = x['raw'].get('context_compaction_events', 0) == 0
    checks['valid_measurement_duration'] = math.isfinite(seconds) and seconds > 0
    # Recovery is sampled after cleanup. Publish a rate only when that numerator is proven
    # to represent naturally drained work in this measurement. A latency SLO miss or an
    # explicitly classified load rejection alone does not invalidate an observed rate.
    throughput_requirements = ['drained','all_responded','transport_healthy','raw_integrity',
        'recovered','starts_responded','controls_ready','replies_complete','runtime_error_free',
        'no_shutdown_timeout','known_terminal_statuses','no_unplanned_compaction',
        'valid_measurement_duration']
    result['throughput_ineligible_reasons'] = [key for key in throughput_requirements if not checks[key]]
    result['throughput_eligible'] = not result['throughput_ineligible_reasons']
    if result['throughput_eligible']:
        result['completed_turns_per_second'] = roots.get('Completed',0)/seconds
        result['settled_turns_per_second'] = sum(roots.values())/seconds
    result['throughput_scope'] = 'Published only with valid raw and recovery evidence, complete replies, healthy transport, natural drain, successful lifecycle checks, and positive measured duration. Recovered counts remain separate when ineligible. Latency SLO misses and classified load rejections alone do not suppress a valid observed rate; a rate is not proof of sustainable capacity.'
    result['integrity_errors'] = errors
    result['empirical_slo_and_drain_pass'] = all(checks.values())
    result['sustainability_scope'] = 'Finite-run empirical latency and end drain only. Inspect backlog/resource time series; this flag alone is not proof of long-term stability or maximum capacity.'
    result['io_scope'] = 'Kernel-attributed I/O for the test process, including Core, local provider and harness. Not SQLite-only writes, fsync latency or device/NAND bytes.'
    result['queue_scope'] = 'Per-stage outstanding reservations, some include execution. Never sum stages as business backlog; oldest is sampled and omitted means a lower bound.'
    result['workload_evidence'] = {k:x['raw'].get(k) for k in ['provider_requests','provider_request_bytes','output_events','output_bytes','context_compaction_events']}
    result['workload_evidence'].update({k:x.get(k) for k in ['provider_output_bytes','fixture_context_window_tokens','logical_turn_limit','mixed_workload']})
    result['error_classes'] = dict(collections.Counter(json.dumps(r['response']['error'],sort_keys=True,ensure_ascii=False) for r in records if (r.get('response') or {}).get('error')))
    starts_raw = [r for r in records if r['operation']=='agent.startConversationTurn']
    terminal = x['raw']['terminal']
    owner_facts = {}
    for owner in range(x['case']['roots']):
        rows = [r for r in starts_raw if r['owner']==owner]
        accepted = [r for r in rows if (r.get('response') or {}).get('result',{}).get('runId')]
        status = collections.Counter(terminal.get(r['response']['result']['runId'],{}).get('status','missing') for r in accepted)
        owner_facts[str(owner)] = {'offered':len(rows),'accepted':len(accepted),'observed_terminal':dict(status)}
    result['owner_counts'] = owner_facts
    # Rebuild outstanding RPCs and accepted roots from timestamps. A 10-second series includes
    # inter-burst maxima; taking only the 1-second resource instant can miss whole short bursts.
    origin = min(r['scheduled_us'] for r in starts_raw)
    end = origin + int(x['measured_us'])
    boundaries = list(range(origin,end+1,10_000_000)) + [end]
    changes = []
    for r in records:
        changes.append((r['offered_us'],1,0))
        if r.get('responded_us') is not None: changes.append((r['responded_us'],-1,0))
    for r in starts_raw:
        run = (r.get('response') or {}).get('result',{}).get('runId')
        if run:
            # Admission can precede ACK. Here this is explicitly ACK-to-Done outstanding roots.
            ack = r['responded_us']; done = terminal.get(run,{}).get('at_us')
            if done is None or done >= ack:
                changes.append((ack,0,1))
                if done is not None: changes.append((done,0,-1))
    changes.sort(key=lambda v:(v[0],-v[1],-v[2])); cursor=0; rpc_count=0; root_count=0; timeline=[]
    for at in boundaries:
        rpc_peak=rpc_count; root_peak=root_count
        while cursor<len(changes) and changes[cursor][0]<=at:
            _,rpc_delta,root_delta=changes[cursor];cursor+=1
            rpc_count+=rpc_delta;root_count+=root_delta
            rpc_peak=max(rpc_peak,rpc_count);root_peak=max(root_peak,root_count)
        timeline.append({'elapsed_s':round((at-origin)/1e6,3),'rpc_unanswered':rpc_count,
            'acknowledged_roots_without_done':root_count,'interval_rpc_peak':rpc_peak,'interval_root_peak':root_peak})
    result['backlog_timeline']=timeline
    result['backlog_scope']='Actual RPC offers-to-response and successful admission ACK-to-observed Done only, not all durable/internal work. Terminal before ACK is already settled. Queue and pre-cleanup durable trace counts are complementary evidence. Last boundary is scheduled origin plus measured duration, a roughly 25ms loose upper bound; equal-microsecond ties conservatively count offers before responses.'
    result['lifecycle']={'database_open_ms':x['database_open_us']/1000,'service_startup_ms':x['service_startup_us']/1000,'reopen_ms':x['recovery']['open_service_us']/1000}
    result['stage_metrics']=x['stage_metrics']
    result['resource_timeline']=[{'at_us':v['at_us'],'active_turns':v['active_turns'],'disk':v['disk'],'process_io':v['usage'].get('process_io'),'queue_reservations':v['queues']} for v in samples]
    return result

if __name__=='__main__':
    if sys.argv[1] == '--self-test':
        fixture=read_cases(sys.argv[2])[0]
        # Add the new evidence to a legacy healthy fixture without altering its measured
        # workload. This keeps the regression usable with archived matrix results.
        starts=[r for r in fixture['raw']['rpcs'].values()
            if r['operation']=='agent.startConversationTurn' and (r.get('response') or {}).get('result',{}).get('runId')]
        fixture['recovery']['roots']={r['response']['result']['runId']:{
            'owner':r['owner'],'assistant_message_id':r['response']['result']['assistantMessageId'],
            'terminal_status':fixture['raw']['terminal'][r['response']['result']['runId']]['status']} for r in starts}
        fixture['recovery']['acknowledged_roots_checked']=len(starts)
        fixture['recovery']['acknowledged_root_identity_errors']=[]
        fixture['raw']['transport_failure']=None
        fixture['outbound']['state']={'closed':False,'failed':False,'overloaded':False,'failure_snapshot':None}
        valid=analyze(fixture)
        assert valid['empirical_slo_and_drain_pass'], valid['sustainability_checks']
        assert valid['throughput_eligible'] and valid['completed_turns_per_second'] is not None
        tested=[]
        def rejects(label, altered, reason):
            result=analyze(altered)
            assert not result['empirical_slo_and_drain_pass'],label
            assert not result['throughput_eligible'],label
            assert reason in result['throughput_ineligible_reasons'],(label,result['throughput_ineligible_reasons'])
            assert result['completed_turns_per_second'] is None,label
            assert result['settled_turns_per_second'] is None,label
            tested.append(label)
        for key,value,reason in [('errors',[{'fixture':'injected'}],'runtime_error_free'),
                ('context_compaction_events',1,'no_unplanned_compaction'),
                ('transport_failure',{},'transport_healthy')]:
            altered=copy.deepcopy(fixture);altered['raw'][key]=value
            rejects(key,altered,reason)
        for key,value,reason in [('shutdown_timed_out',True,'no_shutdown_timeout'),
                ('drained_before_cleanup',False,'drained'),('starts_responded',False,'starts_responded'),
                ('controls_ready',False,'controls_ready'),('replies_complete',False,'replies_complete'),
                ('measured_us',0,'valid_measurement_duration')]:
            altered=copy.deepcopy(fixture);altered[key]=value
            rejects(key,altered,reason)
        for key in ['closed','failed','overloaded']:
            altered=copy.deepcopy(fixture);altered['outbound']['state'][key]=True
            rejects(key,altered,'transport_healthy')
        altered=copy.deepcopy(fixture);altered['outbound']['state']['failure_snapshot']={'byte_limit':True}
        rejects('failure_snapshot',altered,'transport_healthy')
        altered=copy.deepcopy(fixture);del altered['raw']['transport_failure']
        rejects('partial_transport_evidence',altered,'transport_healthy')
        altered=copy.deepcopy(fixture);altered['recovered_root_turns']+=1
        rejects('recovery_count',altered,'recovered')
        altered=copy.deepcopy(fixture);altered['recovery']['post_reopen_in_progress']=1
        rejects('recovery_unfinished',altered,'recovered')
        run=next(iter(fixture['recovery']['roots']))
        altered=copy.deepcopy(fixture)
        altered['recovery']['roots']['replacement-run']=altered['recovery']['roots'].pop(run)
        rejects('equal_count_wrong_run',altered,'raw_integrity')
        for key,value in [('owner',fixture['case']['roots']),('assistant_message_id','wrong-message'),
                ('terminal_status','in_progress')]:
            altered=copy.deepcopy(fixture);altered['recovery']['roots'][run][key]=value
            rejects('identity_'+key,altered,'raw_integrity')
        altered=copy.deepcopy(fixture)
        first=next(iter(altered['raw']['rpcs'].values()))
        first['response']=None;first['responded_us']=None
        rejects('missing_response',altered,'all_responded')
        # SLO failure must not be confused with an invalid measured rate.
        altered=copy.deepcopy(fixture);altered['measured_us']+=10_000_000
        for record in altered['raw']['rpcs'].values():record['responded_us']+=10_000_000
        for terminal in altered['raw']['terminal'].values():terminal['at_us']+=10_000_000
        slow=analyze(altered)
        assert not slow['sustainability_checks']['send_p95_under_500ms']
        assert slow['throughput_eligible'],slow['throughput_ineligible_reasons']
        assert slow['completed_turns_per_second'] is not None
        assert not slow['empirical_slo_and_drain_pass']
        print(json.dumps({'self_test':'passed','invalid_cases':tested,
            'invalid_case_count':len(tested),'healthy_case':'passed','slow_but_valid_rate':'passed'}))
    else:
        result=[analyze(x) for x in read_cases(sys.argv[1])]
        pathlib.Path(sys.argv[2]).write_text(json.dumps(result,indent=2)+'\n')
