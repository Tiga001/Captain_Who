import assert from 'node:assert/strict'
import test from 'node:test'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  baselineEnvironment,
  cargoTestExecutable,
  manifestSourcePath,
  inspectResults,
  validateBaselineCase,
  parseMacThreadCount,
  parseOptions,
  parseResourceSample
} from './run-backend-concurrency-baseline.mjs'

test('options reject ambiguous modes, duplicate arguments and invalid waves', () => {
  for (const args of [
    ['--mode', 'fast'],
    ['--waves', '0'],
    ['--waves', '901'],
    ['--waves', '1.5'],
    ['--mode'],
    ['--oops', '1'],
    ['--filter', 'a', '--filter', 'b']
  ])
    assert.throws(() => parseOptions(args, '/repo'))
  assert.equal(parseOptions(['--mode', 'full'], '/repo').waves, 3)
  const options = parseOptions(
    ['--mode', 'smoke', '--waves', '9', '--binary', 'bin/test', '--output-dir', 'result'],
    '/repo'
  )
  assert.equal(options.waves, 9)
  assert.equal(options.binary, '/repo/bin/test')
  assert.equal(options.outputDir, '/repo/result')
})

test('baseline build is bounded and isolated; old filter cannot narrow the requested run', () => {
  const options = parseOptions([], '/repo')
  const env = baselineEnvironment(
    options,
    { BACKEND_BASELINE_FILTER: 'stale', CARGO_BUILD_JOBS: '100', CARGO_INCREMENTAL: '1' },
    '/repo'
  )
  assert.equal(env.CARGO_BUILD_JOBS, '4')
  assert.equal(env.CARGO_INCREMENTAL, '0')
  assert.equal(env.CAPTAIN_PERFORMANCE_DIAGNOSTICS, '1')
  assert.equal(
    baselineEnvironment(parseOptions(['--diagnostics', 'off'], '/repo'), {}, '/repo')
      .CAPTAIN_PERFORMANCE_DIAGNOSTICS,
    '0'
  )
  assert.throws(() => parseOptions(['--diagnostics', 'maybe'], '/repo'))
  assert.equal(env.BACKEND_BASELINE_FILTER, undefined)
  assert.equal(env.CARGO_PROFILE_RELEASE_DEBUG, '0')
  assert.throws(
    () => baselineEnvironment(options, { CARGO_TARGET_DIR: '/repo/target' }, '/repo'),
    /isolated/
  )
  assert.throws(
    () => baselineEnvironment(options, { CARGO_TARGET_DIR: '/repo/target/release' }, '/repo'),
    /isolated/
  )
  assert.equal(
    baselineEnvironment(
      { ...options, filter: 'tree' },
      { CARGO_TARGET_DIR: '/tmp/isolated' },
      '/repo'
    ).BACKEND_BASELINE_FILTER,
    'tree'
  )
})

test('source manifests include new Rust/TypeScript but exclude user screenshots and caches', () => {
  for (const path of [
    'crates/core/src/new.rs',
    'src/main/new.ts',
    'src/renderer/Panel.tsx',
    'scripts/runner.mjs',
    'Cargo.lock',
    'pnpm-lock.yaml'
  ])
    assert.equal(manifestSourcePath(path), true, path)
  for (const path of [
    '.cache/secret.ts',
    'assets/screenshots_En/a.png',
    'src/app/__screenshots__/a.json',
    'crates/target/generated.rs',
    '../secret.rs',
    'docs/performance/result.json',
    'src/photo.png'
  ])
    assert.equal(manifestSourcePath(path), false, path)
})

test('Cargo discovery accepts only the requested test executable artifact', () => {
  const good = {
    reason: 'compiler-artifact',
    profile: { test: true },
    target: { name: 'core-server', kind: ['bin'] },
    executable: '/tmp/isolated/core-server-test'
  }
  assert.equal(cargoTestExecutable(good), good.executable)
  assert.equal(cargoTestExecutable({ ...good, profile: { test: false } }), null)
  assert.equal(cargoTestExecutable({ ...good, target: { name: 'different', kind: ['bin'] } }), null)
  assert.equal(cargoTestExecutable({ ...good, executable: 'relative' }), null)
  assert.equal(cargoTestExecutable({ reason: 'build-finished', success: true }), null)
})

test('resource samples never infer unsupported thread counts', () => {
  assert.deepEqual(parseResourceSample(' 1234  12\n', true), {
    rss_bytes: 1234 * 1024,
    threads: 12
  })
  assert.deepEqual(parseResourceSample(' 1234\n', false), { rss_bytes: 1234 * 1024, threads: null })
  for (const value of ['', 'rss', '1234 nope', '1234 12 9'])
    assert.equal(parseResourceSample(value, true), null)
})

test('macOS thread samples count continuation rows only for the requested child', () => {
  const output =
    'USER PID TT %CPU STAT PRI STIME UTIME COMMAND\nrunner 1234 ?? 0.0 S 31 0.1 0.1 child\n          0.0 S 31 0.1 0.1\n'
  assert.equal(parseMacThreadCount(output, 1234), 2)
  assert.equal(parseMacThreadCount(output, 5678), null)
  assert.equal(parseMacThreadCount('PID COMMAND\n1234 child', 1234), null)
  assert.equal(parseMacThreadCount('', 1234), null)
})

// eslint-disable-next-line @typescript-eslint/explicit-function-return-type -- Plain JavaScript fixture builder.
function validCase(work = 'text') {
  return {
    kind: 'backend_concurrency_case',
    name: `${work}-synthetic`,
    case: { work, units: 5, roots: 5 },
    root_starts_durably_accepted: 5,
    recovered_root_turns: 5,
    control_rejections_observed: false,
    recovery: { root_terminal_statuses: work === 'stop' ? { Cancelled: 5 } : { Completed: 5 } },
    raw: { provider_first_us: {} }
  }
}

test('case postconditions reject lost durable acknowledgements even after empty recovery', () => {
  const valid = validCase()
  assert.deepEqual(validateBaselineCase(valid), [])
  const missing = { ...valid, recovered_root_turns: 0, recovery: { root_terminal_statuses: {} } }
  assert.ok(validateBaselineCase(missing).includes('recovered_root_count_mismatch'))
  assert.ok(validateBaselineCase(missing).includes('root_terminal_count_mismatch'))
  assert.ok(
    validateBaselineCase({ ...valid, recovered_root_turns: 6 }).includes(
      'recovered_root_count_mismatch'
    )
  )
  assert.ok(
    validateBaselineCase({ ...valid, root_starts_durably_accepted: 0 }).includes(
      'invalid_accepted_root_count'
    )
  )
})

test('non-stop runs must complete and stop uses the actual Rust Cancelled variant', () => {
  assert.deepEqual(validateBaselineCase(validCase('stop')), [])
  const cancelledText = { ...validCase(), recovery: { root_terminal_statuses: { Cancelled: 5 } } }
  assert.ok(validateBaselineCase(cancelledText).includes('unexpected_root_terminal_status'))
  const completedStop = {
    ...validCase('stop'),
    recovery: { root_terminal_statuses: { Completed: 5 } }
  }
  assert.ok(validateBaselineCase(completedStop).includes('unexpected_root_terminal_status'))
  const misspelled = { ...validCase('stop'), recovery: { root_terminal_statuses: { Canceled: 5 } } }
  assert.ok(validateBaselineCase(misspelled).includes('invalid_root_terminal_distribution'))
})

test('overload permits labelled cleanup cancellations but still requires exact durable accounting', () => {
  const base = { ...validCase('approval'), control_rejections_observed: true }
  assert.deepEqual(
    validateBaselineCase({
      ...base,
      recovery: { root_terminal_statuses: { Completed: 2, Cancelled: 3 } }
    }),
    []
  )
  for (const distribution of [
    { Completed: 2, Cancelled: 2 },
    { Completed: 2, Failed: 3 },
    { Completed: 2, InProgress: 3 },
    { Completed: 2, Cancelled: 3.5 }
  ]) {
    assert.notDeepEqual(
      validateBaselineCase({ ...base, recovery: { root_terminal_statuses: distribution } }),
      []
    )
  }
  assert.ok(
    validateBaselineCase({ ...base, control_rejections_observed: undefined }).includes(
      'missing_control_rejection_state'
    )
  )
})

test('Tree requires each expected child provider marker, not only node population', () => {
  const tree = {
    ...validCase('tree'),
    case: { work: 'tree', units: 5, roots: 2 },
    root_starts_durably_accepted: 2,
    recovered_root_turns: 2,
    recovery: { root_terminal_statuses: { Completed: 2 } },
    raw: {
      provider_first_us: {
        'root-0-wave-0': 1,
        'root-1-wave-0': 2,
        'root-0-child-0': 3,
        'root-0-child-1': 4,
        'root-1-child-0': 5
      }
    }
  }
  assert.deepEqual(validateBaselineCase(tree), [])
  assert.ok(
    validateBaselineCase({ ...tree, raw: { provider_first_us: { 'root-0-child-0': 3 } } }).includes(
      'child_provider_marker_mismatch'
    )
  )
  const wrongOwner = { 'root-0-child-0': 3, 'root-0-child-1': 4, 'root-9-child-0': 5 }
  assert.ok(
    validateBaselineCase({ ...tree, raw: { provider_first_us: wrongOwner } }).includes(
      'child_provider_marker_mismatch'
    )
  )
  assert.ok(
    validateBaselineCase({ ...tree, case: { work: 'tree', units: 51, roots: 2 } }).includes(
      'invalid_tree_population'
    )
  )
})

test('result inspection reports valid JSON with invalid correctness as integrity failure', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'captain-baseline-validation-'))
  try {
    const path = join(directory, 'cases.jsonl')
    const bad = { ...validCase(), recovered_root_turns: 0 }
    await writeFile(path, `${JSON.stringify(validCase())}\n${JSON.stringify(bad)}\n`)
    const result = await inspectResults(path)
    assert.equal(result.cases, 2)
    assert.equal(result.invalidLines, 0)
    assert.equal(result.missing, false)
    assert.deepEqual(result.integrityFailures, [
      { case: bad.name, errors: ['recovered_root_count_mismatch'] }
    ])
    await writeFile(path, `${JSON.stringify(validCase('stop'))}\n`)
    assert.deepEqual(await inspectResults(path), {
      cases: 1,
      invalidLines: 0,
      integrityFailures: [],
      missing: false
    })
    await writeFile(path, 'not JSON\nnull\n')
    assert.equal((await inspectResults(path)).invalidLines, 2)
    assert.deepEqual(await inspectResults(join(directory, 'missing.jsonl')), {
      cases: 0,
      invalidLines: 0,
      integrityFailures: [],
      missing: true
    })
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})

test('soak parameters are explicit, bounded, and cannot alter the old matrix', () => {
  const options = parseOptions(
    ['--mode', 'soak', '--duration-seconds', '600', '--units', '20', '--interval-ms', '1000'],
    '/repo'
  )
  assert.equal(options.durationSeconds, 600)
  assert.equal(options.intervalMs, 1000)
  const environment = baselineEnvironment(options, {}, '/repo')
  assert.equal(environment.BACKEND_BASELINE_DURATION_SECONDS, '600')
  assert.equal(environment.BACKEND_BASELINE_UNITS, '20')
  for (const args of [
    ['--mode', 'full', '--units', '20'],
    ['--mode', 'soak', '--duration-seconds', '1801'],
    ['--mode', 'soak', '--interval-ms', '249'],
    ['--mode', 'soak', '--units', '21']
  ])
    assert.throws(() => parseOptions(args, '/repo'))
})

test('raw RPC outcomes must agree with aggregate metrics and recovery', () => {
  const record = {
    operation: 'agent.startConversationTurn',
    response: { result: { runId: 'r' } },
    responded_us: 1
  }
  const value = {
    ...validCase(),
    raw: { rpcs: { 1: record } },
    metrics: { 'agent.startConversationTurn': { offered: 1, success_response: 1 } },
    recovery: { root_terminal_statuses: { Completed: 5 }, post_reopen_in_progress: 0 }
  }
  assert.deepEqual(validateBaselineCase(value), [])
  assert.ok(
    validateBaselineCase({
      ...value,
      raw: { rpcs: { 1: { ...record, response: null } } }
    }).includes('missing_rpc_response')
  )
  assert.ok(
    validateBaselineCase({
      ...value,
      metrics: { 'agent.startConversationTurn': { offered: 2, success_response: 1 } }
    }).includes('rpc_count_mismatch:agent.startConversationTurn')
  )
  assert.ok(
    validateBaselineCase({ ...value, unexpected_rpc_errors: [{}] }).includes(
      'unexpected_rpc_errors'
    )
  )
  assert.ok(
    validateBaselineCase({
      ...value,
      recovery: { ...value.recovery, post_reopen_in_progress: 1 }
    }).includes('recovery_unfinished')
  )
})

test('version two requires planned work and accepted starts to match the raw evidence', () => {
  const value = {
    ...validCase(),
    format_version: 2,
    case: { work: 'text', units: 1, roots: 1, waves: 1 },
    root_starts_durably_accepted: 1,
    recovered_root_turns: 1,
    recovery: { root_terminal_statuses: { Completed: 1 }, post_reopen_in_progress: 0 },
    raw: {
      rpcs: {
        1: {
          operation: 'agent.startConversationTurn',
          response: { result: { runId: 'r' } },
          dispatch_admitted: true,
          responded_us: 1
        }
      }
    },
    metrics: {
      'agent.startConversationTurn': {
        offered: 1,
        success_response: 1,
        dispatch_admitted: 1,
        rejected_or_error: 0,
        no_response: 0
      }
    }
  }
  assert.deepEqual(validateBaselineCase(value), [])
  assert.ok(
    validateBaselineCase({ ...value, root_starts_durably_accepted: 5 }).includes(
      'accepted_start_count_mismatch'
    )
  )
  assert.ok(
    validateBaselineCase({ ...value, case: { ...value.case, waves: 2 } }).includes(
      'planned_start_count_mismatch'
    )
  )
  assert.ok(
    validateBaselineCase({ ...value, metrics: {} }).includes(
      'missing_operation:agent.startConversationTurn'
    )
  )
  const mixed = {
    ...value,
    fixed_wave_interval_ms: 2000,
    case: { ...value.case, work: 'mixed' },
    raw: { rpcs: { 1: { ...value.raw.rpcs[1], provider_key: 'root-0-wave-3' } } }
  }
  assert.ok(validateBaselineCase(mixed).includes('mixed_missing_controls:agent.approveAction'))
  assert.ok(validateBaselineCase(mixed).includes('planned_background_read_count_mismatch'))
  assert.ok(validateBaselineCase({ ...value, raw: {} }).includes('missing_raw_rpcs'))
})
