#!/usr/bin/env python3
"""Audit and compare the original 36-case round-3/round-4 concurrency matrix.

Usage: analyze-round4.py ROUND4_DIR [--baseline ROUND3_DIR] [--output-prefix PATH]
       analyze-round4.py --self-test

Read-only inputs. Outputs PATH.json/.csv/.txt only when explicitly requested. RPC quantiles
are exact nearest-rank successful-response samples; histogram quantiles are upper bounds.
Changed accepted/error/terminal counts are reported, never forced equal: different interleaving
under the unchanged 50-Turn limit can legitimately change busy/control outcomes. Every run must
independently reconcile its actual accepted starts with reopened durable terminals.
"""
import argparse
import copy
import csv
import gzip
import hashlib
import json
import math
from collections import Counter
from pathlib import Path

REPO = next(parent for parent in Path(__file__).resolve().parents
            if (parent / 'Cargo.toml').is_file() and (parent / 'docs').is_dir())
BASELINE = REPO / 'docs/performance/backend-concurrency/results/2026-10-11-round3'
COUNTERS = ('offered', 'dispatch_admitted', 'success_response', 'rejected_or_error', 'no_response',
            'busy_or_capacity_rejections')
START = 'agent.startConversationTurn'
BOUNDARY = ('Microseconds; Core intake only, no renderer click latency. Exact success-only RPC '
            'quantiles and all-response counts remain separate; n=1 is one observation. Stage '
            'p95_upper_us is a histogram bucket bound. Fixed-rate completion/s is not maximum '
            'throughput. RSS/threads are sampled process-only peaks. A finite owner/slow-tool '
            'fixture cannot prove absence of sustained starvation or unobserved quota overflow.')


def read_lines(directory, name, required=True):
    for path in (directory / name, directory / (name + '.gz')):
        if path.exists():
            with (gzip.open if path.suffix == '.gz' else open)(path, 'rt', encoding='utf-8') as f:
                return [json.loads(line) for line in f if line.strip()], path
    if required:
        raise ValueError(f'missing {name}[.gz]: {directory}')
    return [], None


def dist(values):
    values = sorted(values)
    n = len(values)
    result = {'count': n, 'kind': 'unavailable', 'max_us': values[-1] if n else None}
    if n == 1:
        result.update(kind='single_observation', observed_us=values[0])
    elif n:
        result.update(kind='sample_distribution', p50_us=values[math.ceil(.5*n)-1],
                      p95_us=values[math.ceil(.95*n)-1])
    return result


def success(rpc):
    return rpc.get('response') is not None and 'error' not in rpc['response']


def observed_counts(rpcs):
    return {'offered': len(rpcs), 'dispatch_admitted': sum(bool(r.get('dispatch_admitted')) for r in rpcs),
            'success_response': sum(success(r) for r in rpcs),
            'rejected_or_error': sum(r.get('response') is not None and 'error' in r['response'] for r in rpcs),
            'no_response': sum(r.get('response') is None for r in rpcs)}


def summarise_case(case):
    violations = []
    raw = case.get('raw', {})
    rpcs = list(raw.get('rpcs', {}).values())
    operations = {}
    for op in sorted(set(case.get('metrics', {})) | {r['operation'] for r in rpcs}):
        rows = [r for r in rpcs if r['operation'] == op]
        counts = observed_counts(rows)
        declared = case.get('metrics', {}).get(op, {})
        for key, value in counts.items():
            if declared.get(key) != value:
                violations.append(f'{op}: {key} raw={value}, declared={declared.get(key)}')
        good = [r for r in rows if success(r) and r.get('responded_us') is not None]
        if len(good) != counts['success_response']:
            violations.append(f'{op}: success lacks response timestamp')
        operations[op] = {
            'counts': counts | {'busy_or_capacity_rejections': declared.get('busy_or_capacity_rejections')},
            'success_scheduled_to_response': dist([r['responded_us']-r['scheduled_us'] for r in good]),
            'success_actual_offer_to_response': dist([r['responded_us']-r['offered_us'] for r in good]),
            'all_response_scheduled_to_response': dist([r['responded_us']-r['scheduled_us'] for r in rows if r.get('responded_us') is not None]),
            'queue_all_started': dist([r['handler_started_us']-r['offered_us'] for r in rows if r.get('handler_started_us') is not None]),
            'errors': dict(Counter(json.dumps(r['response']['error'], ensure_ascii=False, sort_keys=True)
                                   for r in rows if r.get('response') and 'error' in r['response'])),
        }
    for rpc in rpcs:
        if rpc.get('responded_us') is not None and rpc['responded_us'] < rpc['offered_us']:
            violations.append('response predates offer')
    recovery = case.get('recovery', {})
    root_status = recovery.get('root_terminal_statuses', {})
    child_status = recovery.get('child_terminal_statuses', {})
    starts = [r for r in rpcs if r['operation'] == START and success(r)]
    accepted_ids = [r['response'].get('result', {}).get('runId') for r in starts]
    if None in accepted_ids or len(set(accepted_ids)) != len(accepted_ids):
        violations.append('successful root starts have missing/duplicate Run identities')
    accepted = case.get('root_starts_durably_accepted')
    reopened = case.get('recovered_root_turns')
    if accepted != len(starts) or reopened != accepted or sum(root_status.values()) != reopened:
        violations.append(f'root conservation: success={len(starts)}, accepted={accepted}, reopened={reopened}, terminals={root_status}')
    if sum(child_status.values()) != case.get('recovered_child_turns'):
        violations.append('child reopened count differs from durable terminal distribution')
    for key in ('replies_complete', 'starts_responded', 'controls_ready'):
        if case.get(key) is not True:
            violations.append(f'{key} != true')
    for key in ('unexpected_rpc_errors', 'shutdown_timed_out'):
        if case.get(key):
            violations.append(f'{key}={case[key]}')
    if raw.get('errors') or raw.get('context_compaction_events'):
        violations.append('runtime errors or unplanned context compaction')
    if recovery.get('post_reopen_in_progress') != 0:
        violations.append('reopen left in-progress turns')
    if case.get('logical_turn_limit') != 50:
        violations.append(f'logical Turn quota is {case.get("logical_turn_limit")}, expected unchanged 50')
    if case.get('logical_turn_peak') is not None and case['logical_turn_peak'] > 50:
        violations.append(f'observed logical Turn peak exceeds quota: {case["logical_turn_peak"]}')
    stages = {}
    for stat in case.get('stage_metrics', []):
        key = stat['stage'] + '/' + stat['operation']
        if key in stages:
            violations.append(f'duplicate stage histogram {key}')
        stages[key] = {k: stat.get(k) for k in ('count', 'total_us', 'max_us', 'p95_upper_us')}
        stages[key]['kind'] = 'single_observation' if stat.get('count') == 1 else 'histogram_bucket_upper_bound'
        if stat['stage'] == 'diagnostics.omitted' and stat.get('count', 0):
            violations.append('stage diagnostic samples omitted')
    if any(q.get('omitted', 0) for q in case.get('queue_metrics', [])):
        violations.append('queue diagnostic samples omitted')
    owners = {}
    for owner in sorted({str(r.get('owner')) for r in rpcs}):
        rows = [r for r in rpcs if str(r.get('owner')) == owner]
        owners[owner] = observed_counts(rows) | {
            'all_response_latency': dist([r['responded_us']-r['scheduled_us'] for r in rows if r.get('responded_us') is not None]),
            'accepted_root_starts': sum(r['operation'] == START and success(r) for r in rows),
        }
    overlap = None
    if case['case'].get('slow_seconds'):
        fixture = case.get('slow_tool_overlap') or {}
        start, end, slow = (fixture.get(k) for k in ('command_started_us', 'command_exited_us', 'slow_run_id'))
        if start is None or end is None or end <= start:
            violations.append('slow tool has no valid observed command interval')
        else:
            approvals = [r for r in rpcs if 'approve' in r['operation'].lower()]
            offered = [r for r in approvals if start <= r['offered_us'] < end]
            independent = [r for r in starts if r['response']['result']['runId'] != slow]
            terminals = raw.get('terminal', {})
            completed = [r for r in independent if start <= (terminals.get(r['response']['result']['runId']) or {}).get('at_us', end) < end]
            overlap = {'declared': fixture, 'observed_duration_us': end-start,
                       'approval_offers_in_interval': len(offered), 'approval_offers_total': len(approvals),
                       'successful_approval_responses_before_slow_exit': sum(success(r) and r.get('responded_us', end) < end for r in offered),
                       'independent_root_starts': len(independent), 'independent_root_terminals_before_slow_exit': len(completed),
                       'unresponded_approvals': observed_counts(approvals)['no_response']}
            if not approvals or len(offered) != len(approvals):
                violations.append('slow-tool overlap fixture did not offer every approval during the command')
    cpu = sum(case['resources_after'][k]-case['resources_before'][k] for k in ('cpu_user_seconds', 'cpu_system_seconds'))
    expected_throughput = root_status.get('Completed', 0) / (case['measured_us']/1e6)
    if not math.isclose(expected_throughput, case['completed_root_turns_per_second'], rel_tol=1e-8, abs_tol=1e-10):
        violations.append('completed throughput differs from durable completed-count/time')
    return {'workload': case['case'], 'operations': operations, 'accepted_root_starts': accepted,
            'reopened_root_turns': reopened, 'reopened_child_turns': case.get('recovered_child_turns'),
            'root_terminal_statuses': root_status, 'child_terminal_statuses': child_status,
            'measured_us': case['measured_us'], 'cpu_seconds': cpu,
            'completed_root_turns_per_second': expected_throughput,
            'timer_lag': dist(raw.get('timer_lag_us', [])), 'stages': stages,
            'queue_peaks': case.get('queue_metrics', []), 'owners': owners, 'slow_tool_overlap': overlap,
            'provider_peak_connections': case.get('provider_peak_connections'),
            'logical_turn_limit': case.get('logical_turn_limit'),
            'observed_logical_turn_peak': case.get('logical_turn_peak'),
            'drained_before_cleanup': case.get('drained_before_cleanup'),
            'pre_close_in_progress': recovery.get('pre_close_in_progress'),
            'post_reopen_in_progress': recovery.get('post_reopen_in_progress'),
            'sqlite': case.get('sqlite'), 'disk_after': case.get('disk_after'),
            'integrity_violations': violations}


def load(directory):
    directory = Path(directory).resolve()
    metadata = json.loads((directory/'metadata.json').read_text())
    raw, path = read_lines(directory, 'cases.jsonl')
    resources, resource_path = read_lines(directory, 'resources.jsonl', False)
    cases = {c['name']: summarise_case(c) for c in raw}
    violations = [f'{name}: {v}' for name, c in cases.items() for v in c['integrity_violations']]
    if len(cases) != len(raw): violations.append('duplicate case names')
    if len(cases) != 36: violations.append(f'original full matrix requires 36 cases, got {len(cases)}')
    if metadata.get('status') != 'completed' or metadata.get('run', {}).get('code') != 0:
        violations.append('runner did not complete with exit code 0')
    if metadata.get('source_changed_during_run') is not False: violations.append('source stability unverified')
    result = metadata.get('results', {})
    if any(result.get(k) for k in ('missing', 'invalidLines', 'integrityFailures')):
        violations.append('runner reports incomplete or invalid cases')
    rpc, roots, children = Counter(), Counter(), Counter()
    for c in cases.values():
        for op in c['operations'].values(): rpc.update({k: v for k,v in op['counts'].items() if v is not None})
        roots.update(c['root_terminal_statuses']); children.update(c['child_terminal_statuses'])
    return {'directory': str(directory), 'cases_sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'source': {k: metadata.get('source', {}).get(k) for k in ('head', 'source_manifest_sha256', 'tracked_diff_sha256')},
            'run_duration_ms': metadata.get('run', {}).get('duration_ms'),
            'resource_sample_count': len(resources), 'totals': {
                'rpc': dict(rpc), 'root_terminal_statuses': dict(roots), 'child_terminal_statuses': dict(children),
                'accepted_root_starts': sum(c['accepted_root_starts'] for c in cases.values()),
                'reopened_root_turns': sum(c['reopened_root_turns'] for c in cases.values()),
                'reopened_child_turns': sum(c['reopened_child_turns'] for c in cases.values()),
                'cpu_seconds': sum(c['cpu_seconds'] for c in cases.values()),
                'sampled_rss_peak_bytes': max((r['rss_bytes'] for r in resources if r.get('rss_bytes') is not None), default=None),
                'sampled_thread_peak': max((r['threads'] for r in resources if r.get('threads') is not None), default=None),
                'post_reopen_in_progress': sum(c['post_reopen_in_progress'] for c in cases.values()),
                'pre_close_in_progress': sum(c['pre_close_in_progress'] for c in cases.values()),
                'not_drained_before_cleanup': [n for n,c in cases.items() if not c['drained_before_cleanup']]},
            'cases': cases, 'integrity_violations': violations}


def ratio(a,b): return b/a if a not in (None, 0) and b is not None else None


def compare(before, after):
    violations, changes, pairs = [], [], {}
    if before['cases'].keys() != after['cases'].keys(): violations.append('case sets differ')
    for name in sorted(before['cases'].keys() & after['cases'].keys()):
        a,b = before['cases'][name],after['cases'][name]
        if a['workload'] != b['workload']:
            violations.append(f'{name}: workload differs'); continue
        differences = {}
        for key in ('accepted_root_starts','reopened_root_turns','reopened_child_turns','root_terminal_statuses','child_terminal_statuses'):
            if a[key] != b[key]: differences[key] = {'before':a[key], 'after':b[key]}
        for op in sorted(a['operations'].keys() | b['operations'].keys()):
            ca,cb = a['operations'].get(op,{}).get('counts'),b['operations'].get(op,{}).get('counts')
            if ca != cb: differences['rpc/'+op] = {'before':ca,'after':cb}
        if differences: changes.append({'case':name,'actual_outcome_differences':differences})
        pairs[name] = {'same_workload':True,'same_actual_outcomes':not differences,
                       'cpu_after_over_before':ratio(a['cpu_seconds'],b['cpu_seconds']),
                       'throughput_after_over_before':ratio(a['completed_root_turns_per_second'],b['completed_root_turns_per_second'])}
    return {'format_version':1,'boundary':BOUNDARY,'before':before,'after':after,'comparisons':pairs,
            'comparison_violations':violations,'outcome_changes_requiring_interpretation':changes,
            'acceptance': 'invalid_evidence' if violations or before['integrity_violations'] or after['integrity_violations'] else 'internally_consistent_evidence',
            'claim_limit': 'Integrity success does not by itself establish performance improvement. Changed accepted populations must accompany every latency comparison.'}


def render(report):
    lines = [BOUNDARY, 'acceptance: '+report['acceptance']]
    for label in ('before','after'):
        d = report[label]
        lines += [f'{label}: {d["directory"]}; cases={len(d["cases"])}', json.dumps(d['totals'],ensure_ascii=False,sort_keys=True),
                  'integrity: '+json.dumps(d['integrity_violations'],ensure_ascii=False)]
    lines += ['comparison violations: '+json.dumps(report['comparison_violations']),
              'outcome changes (observations, not forced-equality failures): '+json.dumps(report['outcome_changes_requiring_interpretation'],ensure_ascii=False)]
    for name in report['comparisons']:
        a,b = report['before']['cases'][name],report['after']['cases'][name]
        lines.append(f'{name}: accepted {a["accepted_root_starts"]}->{b["accepted_root_starts"]}; reopened root/child {a["reopened_root_turns"]}/{a["reopened_child_turns"]}->{b["reopened_root_turns"]}/{b["reopened_child_turns"]}; CPU {a["cpu_seconds"]:.6f}->{b["cpu_seconds"]:.6f}; completion/s {a["completed_root_turns_per_second"]:.6f}->{b["completed_root_turns_per_second"]:.6f}')
        for op in sorted(a['operations'].keys() & b['operations'].keys()):
            x,y = a['operations'][op],b['operations'][op]
            lines.append(f'  {op}: counts {x["counts"]}->{y["counts"]}; success latency {x["success_scheduled_to_response"]}->{y["success_scheduled_to_response"]}')
        if b['slow_tool_overlap']: lines.append('  slow overlap: '+json.dumps(b['slow_tool_overlap'],ensure_ascii=False))
    return '\n'.join(lines)+'\n'


def write_csv(report,path):
    fields = ['case','category','operation','metric','before','after','after_over_before','same_actual_outcomes']
    with path.open('w',newline='') as f:
        out=csv.DictWriter(f,fieldnames=fields,lineterminator="\n");out.writeheader()
        def row(name,category,op,metric,a,b):
            out.writerow(dict(case=name,category=category,operation=op,metric=metric,before=a,after=b,
                              after_over_before=ratio(a,b) if isinstance(a,(int,float)) and isinstance(b,(int,float)) else None,
                              same_actual_outcomes=report['comparisons'][name]['same_actual_outcomes']))
        for name in report['comparisons']:
            a,b=report['before']['cases'][name],report['after']['cases'][name]
            for metric in ('measured_us','cpu_seconds','completed_root_turns_per_second','accepted_root_starts','reopened_root_turns','reopened_child_turns'):
                row(name,'case','',metric,a[metric],b[metric])
            for op in sorted(a['operations'].keys() | b['operations'].keys()):
                x,y=a['operations'].get(op,{}),b['operations'].get(op,{})
                for group in ('counts','success_scheduled_to_response','success_actual_offer_to_response','queue_all_started'):
                    for metric in sorted(x.get(group,{}).keys() | y.get(group,{}).keys()):
                        row(name,group,op,metric,x.get(group,{}).get(metric),y.get(group,{}).get(metric))
            for stage in sorted(a['stages'].keys() | b['stages'].keys()):
                for metric in ('count','total_us','max_us','p95_upper_us'):
                    row(name,'stage_histogram',stage,metric,a['stages'].get(stage,{}).get(metric),b['stages'].get(stage,{}).get(metric))


def self_test():
    baseline=load(BASELINE)
    assert not baseline['integrity_violations'],baseline['integrity_violations']
    assert compare(baseline,baseline)['acceptance']=='internally_consistent_evidence'
    changed=copy.deepcopy(baseline)
    name=next(iter(changed['cases']))
    changed['cases'][name]['root_terminal_statuses']={'Cancelled':3}
    result=compare(baseline,changed)
    assert result['outcome_changes_requiring_interpretation'] and not result['comparison_violations']
    raw,_=read_lines(BASELINE,'cases.jsonl')
    bad=copy.deepcopy(raw[0]);bad['logical_turn_limit']=51
    assert any('quota' in v for v in summarise_case(bad)['integrity_violations'])
    bad=copy.deepcopy(raw[0]);bad['root_starts_durably_accepted']+=1
    assert any('conservation' in v for v in summarise_case(bad)['integrity_violations'])
    bad=copy.deepcopy(raw[0]);bad['logical_turn_peak']=51
    assert any('exceeds quota' in v for v in summarise_case(bad)['integrity_violations'])
    assert dist([5])=={'count':1,'kind':'single_observation','max_us':5,'observed_us':5}
    print('self-test passed: archived 36 cases reconcile; differing terminal outcomes are observations; quota enlargement and missing accepted receipts fail.')


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('after',nargs='?');p.add_argument('--baseline',type=Path,default=BASELINE)
    p.add_argument('--output-prefix',type=Path);p.add_argument('--self-test',action='store_true')
    args=p.parse_args()
    if args.self_test: self_test();return
    if not args.after:p.error('ROUND4_DIR is required unless --self-test')
    report=compare(load(args.baseline),load(args.after));txt=render(report)
    if args.output_prefix:
        prefix=args.output_prefix
        paths=[Path(str(prefix)+suffix) for suffix in ('.json','.csv','.txt')]
        if any(path.exists() for path in paths):p.error('refusing to overwrite existing analysis evidence; choose a fresh output prefix')
        prefix.parent.mkdir(parents=True,exist_ok=True)
        paths[0].write_text(json.dumps(report,indent=2,ensure_ascii=False,allow_nan=False)+'\n')
        write_csv(report,paths[1]);paths[2].write_text(txt)
        print(json.dumps({'acceptance':report['acceptance'],'outputs':[str(path) for path in paths]}))
    else: print(txt,end='')
    if report['acceptance']=='invalid_evidence':raise SystemExit(1)


if __name__=='__main__':main()
