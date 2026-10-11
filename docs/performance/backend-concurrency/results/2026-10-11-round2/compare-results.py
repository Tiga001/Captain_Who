#!/usr/bin/env python3
"""Read-only comparison of backend-concurrency baseline directories.

Usage: compare-results.py beforeDir afterDir [--json]
Accepts cases.jsonl or cases.jsonl.gz plus metadata.json/resources.jsonl.
Writes only stdout. No benchmarks, source mutation, or third-party dependencies.
Exact nearest-rank success-only RPC distributions are recomputed from raw records;
all-response/error counts remain separate. A single sample is labelled observation,
never p95. Stage histograms are explicitly bucket upper bounds, not exact quantiles.
"""
import argparse
import gzip
import json
import math
from collections import Counter
from pathlib import Path

COUNTERS = ('offered', 'dispatch_admitted', 'success_response', 'rejected_or_error',
            'no_response', 'busy_or_capacity_rejections')
STAGES = ('rpc.dependency_wait', 'rpc.execution_permit_wait', 'rpc.handler_execution',
          'rpc.blocking_pool_wait', 'sqlite.connection_wait', 'sqlite.connection_hold',
          'turn.child_preparation')


def count_value(value):
    return len(value) if isinstance(value, (list, dict)) else value


def read_jsonl(directory, basename, required=True):
    for path in (directory / basename, directory / (basename + '.gz')):
        if path.exists():
            opener = gzip.open if path.suffix == '.gz' else open
            with opener(path, 'rt', encoding='utf-8') as stream:
                return [json.loads(line) for line in stream if line.strip()]
    if required:
        raise ValueError(f'missing {basename}[.gz] in {directory}')
    return []


def distribution(values):
    values = sorted(values)
    n = len(values)
    out = {'count': n, 'max_us': values[-1] if n else None}
    if n == 1:
        out.update(kind='single_observation', observed_us=values[0])
    elif n:
        out.update(kind='sample_distribution', p50_us=values[math.ceil(n * .50) - 1],
                   p95_us=values[math.ceil(n * .95) - 1])
    else:
        out['kind'] = 'unavailable'
    return out


def normalise_stat(stat):
    if not stat:
        return {'count': 0, 'kind': 'unavailable', 'max_us': None}
    count = stat.get('count', 0)
    if count == 1:
        return {'count': 1, 'kind': 'single_observation',
                'observed_us': stat.get('max_us'), 'max_us': stat.get('max_us')}
    return {k: stat.get(k) for k in ('count', 'p50_us', 'p95_us', 'max_us')} | {
        'kind': 'sample_distribution' if count else 'unavailable'}


def omitted_counts(case):
    return {
        'stage_samples': sum(s.get('count', 0) for s in case.get('stage_metrics', [])
                             if s.get('stage') == 'diagnostics.omitted'),
        'queue_samples': sum(s.get('omitted', 0) for s in case.get('queue_metrics', [])),
    }


def summarise_case(case):
    operations = {}
    raw_rpcs = list(case['raw']['rpcs'].values())
    for operation, metrics in case['metrics'].items():
        rpcs = [r for r in raw_rpcs if r['operation'] == operation]
        successful = [r for r in rpcs if r.get('response') is not None
                      and 'error' not in r['response'] and r.get('responded_us') is not None]
        timed = lambda field: [r['responded_us'] - r[field] for r in successful
                               if r.get(field) is not None]
        operations[operation] = {
            'counts': {key: metrics.get(key) for key in COUNTERS},
            'success_scheduled_to_response': distribution(timed('scheduled_us')),
            'success_actual_offer_to_response': distribution(timed('offered_us')),
            'all_response_scheduled_to_response': normalise_stat(metrics.get('scheduled_to_response')),
            'queue_all_started': normalise_stat(metrics.get('queue')),
            'handler_to_response_all_started': normalise_stat(metrics.get('handler_to_response')),
            'raw_offer_count_matches': len(rpcs) == metrics.get('offered'),
            'raw_success_count_matches': len(successful) == metrics.get('success_response'),
        }
    recovery = case.get('recovery', {})
    cpu = {k: case['resources_after'][k] - case['resources_before'][k]
           for k in ('cpu_user_seconds', 'cpu_system_seconds')}
    stages = {}
    for metric in case.get('stage_metrics', []):
        if metric['stage'] not in STAGES:
            continue
        values = {k: metric.get(k) for k in ('count', 'max_us', 'total_us')}
        if metric['count'] == 1:
            values['observed_us'] = metric.get('max_us')
            values['kind'] = 'single_observation'
        else:
            values['p95_upper_us'] = metric.get('p95_upper_us')
            values['kind'] = 'histogram_bucket_upper_bound'
        stages[metric['stage'] + '/' + metric['operation']] = values
    return {
        'workload': case['case'], 'measured_us': case['measured_us'],
        'operations': operations, 'timer_lag': normalise_stat(case['timer_lag']),
        'cpu_seconds': cpu, 'cpu_total_seconds': sum(cpu.values()),
        'completed_root_turns_per_second': case['completed_root_turns_per_second'],
        'root_starts_durably_accepted': case['root_starts_durably_accepted'],
        'recovered_root_turns': case['recovered_root_turns'],
        'recovered_child_turns': case['recovered_child_turns'],
        'root_terminal_statuses': recovery.get('root_terminal_statuses'),
        'child_terminal_statuses': recovery.get('child_terminal_statuses'),
        'provider_peak_connections': case['provider_peak_connections'],
        'integrity': {key: case.get(key) for key in ('replies_complete', 'starts_responded',
                       'controls_ready', 'unexpected_rpc_errors', 'shutdown_timed_out',
                       'drained_before_cleanup', 'in_progress_before_cleanup')} | {
            'pre_close_in_progress': recovery.get('pre_close_in_progress'),
            'post_reopen_in_progress': recovery.get('post_reopen_in_progress'),
            'context_compaction_event_count': count_value(case['raw'].get('context_compaction_events', 0)),
            'raw_error_count': len(case['raw'].get('errors', [])),
        },
        'omitted': omitted_counts(case), 'stages': stages,
        'queue_peaks': case.get('queue_metrics', []),
        'slow_tool_overlap': case.get('slow_tool_overlap'),
        'execution_access_fixture': case.get('execution_access_fixture'),
    }


def load(directory):
    directory = Path(directory).resolve()
    metadata = json.loads((directory / 'metadata.json').read_text())
    raw_cases = read_jsonl(directory, 'cases.jsonl')
    cases = {c['name']: summarise_case(c) for c in raw_cases}
    if len(cases) != len(raw_cases):
        raise ValueError(f'duplicate case names in {directory}')
    resources = read_jsonl(directory, 'resources.jsonl', required=False)
    rpc = Counter()
    roots = Counter()
    children = Counter()
    violations = []
    for name, case in cases.items():
        for operation, data in case['operations'].items():
            for key, value in data['counts'].items():
                if value is not None:
                    rpc[key] += value
            if not data['raw_offer_count_matches'] or not data['raw_success_count_matches']:
                violations.append(f'{name}/{operation}: raw counter mismatch')
        roots.update(case['root_terminal_statuses'] or {})
        children.update(case['child_terminal_statuses'] or {})
        for key in ('replies_complete', 'starts_responded', 'controls_ready'):
            if case['integrity'][key] is not True:
                violations.append(f'{name}: {key} != true')
        for key in ('unexpected_rpc_errors', 'shutdown_timed_out', 'post_reopen_in_progress',
                    'context_compaction_event_count', 'raw_error_count'):
            if count_value(case['integrity'][key]) not in (0, False):
                violations.append(f'{name}: {key}={case["integrity"][key]}')
        if any(case['omitted'].values()):
            violations.append(f'{name}: diagnostic samples omitted')
    results = metadata.get('results', {})
    if metadata.get('status') != 'completed' or metadata.get('run', {}).get('code') != 0:
        violations.append('runner not completed with exit code 0')
    if metadata.get('source_changed_during_run') is not False:
        violations.append('source stability not verified')
    if results.get('missing') or results.get('invalidLines') or results.get('integrityFailures'):
        violations.append('runner metadata reports integrity failures')
    full = metadata.get('options', {}).get('mode') == 'full' and not metadata.get('options', {}).get('filter')
    if full and len(cases) != 36:
        violations.append(f'full matrix expected 36 cases, got {len(cases)}')
    summary_path = directory / 'summary.json'
    if summary_path.exists():
        archived = json.loads(summary_path.read_text())
        if archived.get('totals', {}).get('rpc') != dict(rpc):
            violations.append('archived summary RPC totals disagree with raw cases')
    return {
        'directory': str(directory), 'source_commit': metadata.get('source', {}).get('head'),
        'source_manifest_sha256': metadata.get('source', {}).get('source_manifest_sha256'),
        'source_changed_during_run': metadata.get('source_changed_during_run'),
        'run_duration_ms': metadata.get('run', {}).get('duration_ms'),
        'case_count': len(cases), 'totals': {
            'rpc': dict(rpc), 'root_turns': sum(c['recovered_root_turns'] for c in cases.values()),
            'child_turns': sum(c['recovered_child_turns'] for c in cases.values()),
            'root_terminal_statuses': dict(roots), 'child_terminal_statuses': dict(children),
            'cpu_seconds': sum(c['cpu_total_seconds'] for c in cases.values()),
            'pre_close_in_progress': sum(c['integrity']['pre_close_in_progress'] for c in cases.values()),
            'post_reopen_in_progress': sum(c['integrity']['post_reopen_in_progress'] for c in cases.values()),
            'not_drained_before_cleanup_cases': [name for name, c in cases.items() if not c['integrity']['drained_before_cleanup']],
            'sampled_rss_peak_bytes': max((r['rss_bytes'] for r in resources if r.get('rss_bytes') is not None), default=None),
            'sampled_thread_peak': max((r['threads'] for r in resources if r.get('threads') is not None), default=None),
            'resource_sample_count': len(resources),
            'omitted': {key: sum(c['omitted'][key] for c in cases.values()) for key in ('stage_samples', 'queue_samples')},
        }, 'integrity_violations': violations, 'cases': cases,
    }


def ratio(before, after):
    return None if before in (None, 0) or after is None else after / before


def compare(before, after):
    pairs = {}
    for name in sorted(before['cases'].keys() & after['cases'].keys()):
        a, b = before['cases'][name], after['cases'][name]
        pairs[name] = {
            'same_workload': a['workload'] == b['workload'],
            'timer_p95_after_over_before': ratio(a['timer_lag'].get('p95_us'), b['timer_lag'].get('p95_us')),
            'timer_max_after_over_before': ratio(a['timer_lag']['max_us'], b['timer_lag']['max_us']),
            'cpu_after_over_before': ratio(a['cpu_total_seconds'], b['cpu_total_seconds']),
            'throughput_after_over_before': ratio(a['completed_root_turns_per_second'], b['completed_root_turns_per_second']),
        }
    return {
        'boundary': 'Microseconds; Core intake only. Success RPC and all responses separate. Never average case quantiles. n=1 is a single observation. Stage p95 values are bucket upper bounds. Fixed-rate completed Turn/s is not maximum throughput. RSS/threads are sampled process-only peaks, not exact peaks or case increments.',
        'before': before, 'after': after, 'matched_case_count': len(pairs),
        'missing_after': sorted(before['cases'].keys() - after['cases'].keys()),
        'new_after': sorted(after['cases'].keys() - before['cases'].keys()), 'comparisons': pairs,
    }


def ms(stat):
    key = 'observed_us' if stat.get('kind') == 'single_observation' else 'p95_us'
    value = stat.get(key)
    return 'unavailable' if value is None else f'{value / 1000:.3f}'


def text_report(report):
    print(report['boundary'])
    for label in ('before', 'after'):
        data = report[label]
        print(f'\n{label}: {data["directory"]}; cases={data["case_count"]}; commit={data["source_commit"]}')
        print(json.dumps(data['totals'], ensure_ascii=False, sort_keys=True))
        print('integrity:', json.dumps(data['integrity_violations'], ensure_ascii=False))
    print('\ncase / operation | success/offered before→after | errors/no-response before→after | success latency ms before→after [statistic] | timer p95/max ms before→after')
    for name in report['before']['cases']:
        if name not in report['after']['cases']:
            continue
        a, b = report['before']['cases'][name], report['after']['cases'][name]
        for op in sorted(a['operations'].keys() | b['operations'].keys()):
            if op not in a['operations'] or op not in b['operations']:
                print(name, op, 'operation missing on one side'); continue
            x, y = a['operations'][op], b['operations'][op]
            cx, cy = x['counts'], y['counts']
            sx, sy = x['success_scheduled_to_response'], y['success_scheduled_to_response']
            kind = f'{sx["kind"].replace("sample_distribution", "sample_p95")}(n={sx["count"]})→{sy["kind"].replace("sample_distribution", "sample_p95")}(n={sy["count"]})'
            print(f'{name} / {op} | {cx["success_response"]}/{cx["offered"]}→{cy["success_response"]}/{cy["offered"]} | '
                  f'{cx["rejected_or_error"]}/{cx["no_response"]}→{cy["rejected_or_error"]}/{cy["no_response"]} | '
                  f'{ms(sx)}→{ms(sy)} [{kind}] | '
                  f'{ms(a["timer_lag"])}/{a["timer_lag"]["max_us"]/1000:.3f}→{ms(b["timer_lag"])}/{b["timer_lag"]["max_us"]/1000:.3f}')
    for label in ('before', 'after'):
        for name, case in report[label]['cases'].items():
            if name.startswith('steady'):
                print(f'{label} steady: {name}; fixed-rate completed Turn/s={case["completed_root_turns_per_second"]:.6f}; '
                      f'CPU seconds={case["cpu_total_seconds"]:.6f}; measured seconds={case["measured_us"]/1e6:.6f}')
    print('missing_after:', report['missing_after'], 'new_after:', report['new_after'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('beforeDir'); parser.add_argument('afterDir')
    parser.add_argument('--json', action='store_true', help='emit full machine-readable evidence to stdout')
    args = parser.parse_args()
    report = compare(load(args.beforeDir), load(args.afterDir))
    if args.json:
        print(json.dumps(report, indent=2, ensure_ascii=False, allow_nan=False))
    else:
        text_report(report)


if __name__ == '__main__':
    main()
