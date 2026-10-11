import hashlib
import json
import pathlib
import re

base = pathlib.Path(__file__).resolve().parent
out = base / 'regressions'
sources = [
    out / 'results-all.json',
    out / 'results-44-47-48-11-37.json',
    out / 'results-47-48.json',
]
by_index = {}
for source in sources:
    for row in json.loads(source.read_text()):
        by_index[row['index']] = row
expected_groups = set(range(49)) - {31, 32}
assert set(by_index) == expected_groups, (set(by_index), expected_groups)
results = [by_index[index] for index in sorted(by_index)]
tests = {}
for row in results:
    assert row['returncode'] == 0, row
    assert row['counts'][0] == 'ok' and int(row['counts'][1]) > 0, row
    assert int(row['counts'][2]) == 0, row
    log = out / row['log']
    content = log.read_text()
    inventory_path = out / ('server-test-inventory-' + row['sha256'][:8] + '.txt')
    inventory = {
        line.removesuffix(': test')
        for line in inventory_path.read_text().splitlines()
        if line.endswith(': test')
    }
    expected_tests = {name for name in inventory if row['filter'] in name}
    logged_tests = set(re.findall(r'^test (\S+) \.\.\.', content, re.M))
    assert logged_tests == expected_tests, (row['index'], logged_tests ^ expected_tests)
    assert len(expected_tests) == sum(map(int, row['counts'][1:4])), row
    row['log_sha256'] = hashlib.sha256(log.read_bytes()).hexdigest()
    for name in sorted(expected_tests):
        entry = tests.setdefault(name, {'name': name, 'groups': [], 'binary_sha256': []})
        entry['groups'].append(row['index'])
        if row['sha256'] not in entry['binary_sha256']:
            entry['binary_sha256'].append(row['sha256'])

manifest = [tests[name] for name in sorted(tests)]
(out / 'results-all-server.json').write_text(json.dumps(results, indent=2) + '\n')
(out / 'server-tests-deduplicated.json').write_text(json.dumps(manifest, indent=2) + '\n')
(out / 'server-tests-deduplicated.txt').write_text('\n'.join(sorted(tests)) + '\n')
summary = {
    'groups': len(results),
    'distinct_tests': len(tests),
    'passed_executions': sum(int(row['counts'][1]) for row in results),
    'failed_executions': sum(int(row['counts'][2]) for row in results),
    'ignored_executions': sum(int(row['counts'][3]) for row in results),
    'binary_sha256': sorted({row['sha256'] for row in results}),
    'duration_sum_s': round(sum(row['duration_s'] for row in results), 3),
    'initial_fixture_failures': 16,
    'initial_failure_evidence': '../diagnostics/server-credential-fixture-first-run',
    'rerun_scope': [47, 48],
    'debug_only_continuous_send': 'not included in release inventory; separate debug run required',
}
(out / 'server-regression-summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print(json.dumps(summary))
