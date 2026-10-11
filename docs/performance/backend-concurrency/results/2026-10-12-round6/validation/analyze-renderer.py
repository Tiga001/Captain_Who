"""Compare fixed, sequential Renderer fixtures without inferring saturated UI SLOs."""
import collections
import json
import math
import pathlib
import sys


def quantiles(values):
    values = sorted(values)
    return {
        'n': len(values),
        **{key: values[max(0, math.ceil(len(values) * q) - 1)] if values else None
           for key, q in [('p50_ms', .5), ('p95_ms', .95), ('p99_ms', .99), ('max_ms', 1)]},
    }


def summarize(path):
    x = json.loads(pathlib.Path(path).read_text())
    groups = {}
    by_run = {row['runId']: row for row in x.get('terminalEvidence', [])}
    for action in ['submit', 'approve', 'stop']:
        rows = [row for row in x['reports'] if row['action'] == action]
        groups[action] = {
            'accepted': sum(row['accepted'] for row in rows),
            'receipt': quantiles([row['receiptMs'] for row in rows]),
            'frame_opportunity': quantiles([row['frameOpportunityMs'] for row in rows]),
        }
    for work in ['text', 'approval', 'stop']:
        rows = [row for row in x['reports'] if row['action'] == 'submit'
                and by_run.get(row['runId'], {}).get('work') == work]
        groups['submit/' + work] = {
            'accepted': sum(row['accepted'] for row in rows),
            'receipt': quantiles([row['receiptMs'] for row in rows]),
            'frame_opportunity': quantiles([row['frameOpportunityMs'] for row in rows]),
        }
    outcomes = collections.Counter()
    samples = {}
    for window in x['hostMetrics']:
        outcomes.update(window['outcomes'])
        for key, sample in window['samples'].items():
            total = samples.setdefault(key, {'count': 0, 'total_ms': 0, 'bytes': 0})
            total['count'] += sample['count']
            total['total_ms'] += sample['totalMs']
            total['bytes'] += sample['bytes']
    def stage_total(stage):
        rows = [value for key, value in samples.items() if key.startswith(stage + ':')]
        return {field: sum(row[field] for row in rows) for field in ['count', 'total_ms', 'bytes']}
    return {
        'path': str(path), 'binary_sha256': x['binarySha256'],
        'status': x['status'],
        **{key: x[key] for key in ['offeredActions', 'completedActions', 'failedActions',
                                  'unfinishedActions', 'verifiedTerminals', 'reopenedAssistantMessages',
                                  'providerRequests', 'verificationReads']},
        'terminal_counts': dict(collections.Counter(row['status'] for row in x['terminalEvidence'])),
        'actions': groups, 'rpc_counts': x['rpcCounts'], 'host_outcomes': dict(outcomes),
        'host_samples': samples,
        'host_outgoing_requests': stage_total('request.serialize'),
        'host_incoming_json_replies_and_notifications': stage_total('response.parse'),
        'host_notification_dispatch': stage_total('notification.dispatch'),
        'host_pending_peak': max(window['pendingPeak'] for window in x['hostMetrics']),
        'host_omitted_traces': sum(window['omittedTraces'] for window in x['hostMetrics']),
        'renderer_omitted_traces': sum(window['omittedTraces'] for window in x['rendererDiagnostics']),
        'terminal_content': [{key: row[key] for key in ['work', 'index', 'status', 'contentBytes', 'contentSha256']}
                             for row in x['terminalEvidence']],
    }


baseline = summarize(sys.argv[1])
current = summarize(sys.argv[2])
result = {
    'baseline': baseline,
    'current': current,
    'same_terminal_content': baseline['terminal_content'] == current['terminal_content'],
    'same_action_counts': all(baseline[key] == current[key] for key in [
        'offeredActions', 'completedActions', 'failedActions', 'unfinishedActions',
        'verifiedTerminals', 'reopenedAssistantMessages', 'providerRequests', 'verificationReads']),
    'quantiles': 'Nearest rank of raw within-Renderer monotonic spans; n=10/30, so p99 is an observed maximum and not a reliable population tail.',
    'scope': 'Sequential low-load programmatic DOM handlers, production hooks, native Electron IPC and Core stdio. Double rAF is a frame opportunity, not proof of compositor paint. Core admission/decision receipt and terminal durability are separately checked.',
    'verification_overhead': 'Host totals include 81 separately counted verification reads and Core shutdown outside timed actions; 1 reopened read belongs to the fresh Core process. These are fixed on both sides.',
    'limits': ['No hardware click latency, saturated Renderer queue, full AppShell render, real Provider latency or physical disk-write comparison.',
               'One run per version cannot establish statistical significance or a production SLO. Do not convert RPC bytes to physical disk writes.',
               'Storage call reduction includes removal of redundant Renderer saves and addition of Core persistence; it is not a complete physical-write accounting.'],
}
pathlib.Path(sys.argv[3]).write_text(json.dumps(result, indent=2) + '\n')
