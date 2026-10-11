import json, pathlib, re, subprocess, time, sys
base = pathlib.Path('.cache/backend-concurrency/round2-validation-20261011')
base.mkdir(parents=True, exist_ok=True)
target = pathlib.Path('/tmp/captain-backend-baseline-20261011-target/release/deps')
suites = [
 ('core', 'trusted_wake_'),
 ('core', 'settlement_clock'),
 ('core', 'storage::agent_graph_repository::tests'),
 ('server', 'application::blocking_executor'),
 ('server', 'application::agent_dispatcher::tests'),
 ('server', 'runtime_isolation_tests'),
 ('server', 'transport::bounded_dispatch'),
 ('server', 'workflow_scheduler::tests'),
 ('server', 'terminal_persistence_retry_tests'),
 ('server', 'application::agent::execution_access'),
 ('server', 'execution_access_expiring_at_storage_commit_rolls_back_messages_trace_and_binding'),
 ('server', 'permission_revoked_after_precheck_is_blocked_atomically_before_humanroot_admission'),
 ('server', 'wake_execution_fence::tests'),
 ('server', 'trusted_terminal_fence_refusal_preserves_shared_usage_accumulator'),
 ('server', 'application::agent::tests::cancellation'),
 ('server', 'application::agent::tests::human_input::'),
 ('server', 'application::agent::tests::human_input_async::'),
 ('server', 'pending_actions::approval_ordering'),
 ('server', 'application::agent::tests::collaboration_harness'),
 ('server', 'destructive_resource_mutations_terminalize_live_automation_runs_before_trace_deletion'),
 ('server', 'project_deletion_timeout_preserves_late_command_observation'),
]
start_index = int(sys.argv[1]) if len(sys.argv) > 1 else 0
results = json.loads((base/'regressions.json').read_text())[:start_index] if start_index else []
for i,(kind,pattern) in enumerate(suites):
    if i < start_index: continue
    binary = target / ('mycopilot_core-0b5932cd02e8a78b' if kind=='core' else 'core_server-a5ca48c145d29d84')
    path=base / ('final-%02d-%s.log' % (i,pattern.replace('::','-')))
    start=time.monotonic()
    with path.open('w') as out:
        p=subprocess.run([str(binary),pattern,'--test-threads=1','--nocapture'],stdout=out,stderr=subprocess.STDOUT,timeout=300)
    text=path.read_text()
    matched=re.search(r'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored',text)
    r=dict(kind=kind,filter=pattern,returncode=p.returncode,duration_s=round(time.monotonic()-start,3),log=path.name,counts=matched.groups() if matched else None)
    results.append(r)
    (base/'regressions.json').write_text(json.dumps(results,indent=2)+'\n')
    print(json.dumps(r),flush=True)
    if p.returncode or not matched or int(matched.group(2)) == 0: break
