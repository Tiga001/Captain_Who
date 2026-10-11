import json, pathlib, re, subprocess, time, sys, hashlib
base=pathlib.Path(__file__).resolve().parent
out=base/'regressions'; out.mkdir(exist_ok=True)
target=pathlib.Path('/tmp/captain-backend-baseline-20261011-target/release/deps')
suites=[
('server','resource_preparation_tests'),
('server','keyed_admission::tests'),
('server','preparing::tests'),
('server','application::agent::execution_access::tests'),
('server','preparing_deletion_tests'),
('server','scoped_deletion_tests'),
('server','fair_preparation_tests'),
('server','application::agent::rpc_resources'),
('server','transport::bounded_dispatch'),
('server','application::blocking_executor'),
('server','application::agent_dispatcher::tests'),
('server','runtime_isolation_tests'),
('server','workflow_scheduler::tests'),
('server','terminal_persistence_retry_tests'),
('server','execution_access_expiring_at_storage_commit_rolls_back_messages_trace_and_binding'),
('server','permission_revoked_after_precheck_is_blocked_atomically_before_humanroot_admission'),
('server','wake_execution_fence::tests'),
('server','trusted_terminal_fence_refusal_preserves_shared_usage_accumulator'),
('server','application::agent::tests::cancellation'),
('server','application::agent::tests::human_input'),
('server','application::agent::tests::collaboration_harness'),
('server','destructive_resource_mutations_terminalize_live_automation_runs_before_trace_deletion'),
('server','project_deletion_timeout_preserves_late_command_observation'),
('server','application::agent::tests::automation_turn'),
('server','provider_transition'),
('server','application::agent::tests::provider_profiles'),
('server','application::agent::tests::human_root_notifications'),
('server','application::agent::tests::pending_actions'),
('server','application::agent_wait::tests'),
('server','occupancy_tests'),
('server','application::agent::tests::context_window_consistency'),
('core','storage::'),
('core','performance::'),
('server','application::agent::tests::mcp_approval_'),
('server','transport::tests::request_loop'),
('server','transport::request_loop'),
('server','workflow_execution'),
('server','runtime_checkpoint'),
('server','child_event_transport'),
('server','agent_support::tests'),
('server','collaboration_authorization'),
('server','application::agent::tests::terminal_events'),
('server','application::agent::tests::skills::'),
]
binaries={'core':target/'mycopilot_core-7748a31bd053ad1f','server':target/'core_server-f7a97b10b9960ccf'}
results=[]
selection=sys.argv[1:] or [str(i) for i in range(len(suites))]
for value in selection:
    i=int(value);kind,pattern=suites[i];binary=binaries[kind]
    path=out/(f'{i:02d}-'+pattern.replace('::','-')+'.log')
    start=time.monotonic()
    with path.open('w') as f:
        p=subprocess.run([str(binary),pattern,'--test-threads='+('4' if kind=='core' else '1'),'--nocapture'],stdout=f,stderr=subprocess.STDOUT,timeout=900)
    content=path.read_text()
    match=re.search(r'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored',content)
    result=dict(index=i,kind=kind,filter=pattern,returncode=p.returncode,duration_s=round(time.monotonic()-start,3),log=path.name,counts=match.groups() if match else None,binary=binary.name,sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
    results.append(result)
    (out/('results-'+('-'.join(selection) if len(selection)<10 else 'all')+'.json')).write_text(json.dumps(results,indent=2)+'\n')
    print(json.dumps(result),flush=True)
sys.exit(1 if any(r['returncode'] or not r['counts'] or int(r['counts'][1]) == 0 for r in results) else 0)
