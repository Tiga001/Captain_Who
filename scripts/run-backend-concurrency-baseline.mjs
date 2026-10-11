/* eslint-disable @typescript-eslint/explicit-function-return-type -- Standalone runner validates CLI and subprocess boundaries. */
import { spawn, spawnSync } from 'node:child_process'
import { createHash, randomUUID } from 'node:crypto'
import { createReadStream, closeSync, openSync, writeSync } from 'node:fs'
import { mkdir, stat, writeFile } from 'node:fs/promises'
import os from 'node:os'
import { dirname, isAbsolute, join, resolve, sep } from 'node:path'
import process from 'node:process'
import { createInterface } from 'node:readline'
import { fileURLToPath, pathToFileURL } from 'node:url'

export const TEST_NAME =
  'application::agent::tests::backend_concurrency_baseline::backend_concurrency_disk_baseline'
const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const defaultTarget = '/tmp/captain-backend-baseline-20261011-target'

export function parseOptions(args, root = repositoryRoot) {
  const values = new Map()
  const supported = new Set([
    '--mode',
    '--filter',
    '--waves',
    '--output-dir',
    '--binary',
    '--duration-seconds',
    '--units',
    '--interval-ms',
    '--diagnostics'
  ])
  for (let index = 0; index < args.length; index += 2) {
    const key = args[index]
    if (!supported.has(key)) throw new Error(`Unknown option: ${key}`)
    const value = args[index + 1]
    if (!value || value.startsWith('--') || value.includes('\0'))
      throw new Error(`Missing or invalid value: ${key}`)
    if (values.has(key)) throw new Error(`Duplicate option: ${key}`)
    values.set(key, value)
  }
  const mode = values.get('--mode') ?? 'smoke'
  const diagnostics = values.get('--diagnostics') ?? 'on'
  if (!['on', 'off'].includes(diagnostics)) throw new Error('--diagnostics must be on or off')
  if (!['smoke', 'full', 'soak'].includes(mode))
    throw new Error('--mode must be smoke, full or soak')
  const rawWaves = values.get('--waves')
  if (rawWaves !== undefined && (!/^\d+$/u.test(rawWaves) || +rawWaves < 1 || +rawWaves > 900))
    throw new Error('--waves must be an integer from 1 to 900')
  const integer = (key, fallback, minimum, maximum) => {
    const raw = values.get(key)
    if (raw !== undefined && (!/^\d+$/u.test(raw) || +raw < minimum || +raw > maximum))
      throw new Error(`${key} must be an integer from ${minimum} to ${maximum}`)
    if (raw !== undefined && mode !== 'soak') throw new Error(`${key} requires soak mode`)
    return raw === undefined ? fallback : +raw
  }
  const units = integer('--units', 20, 1, 50)
  if (![1, 5, 10, 20, 50].includes(units)) throw new Error('--units must be 1/5/10/20/50')
  return {
    mode,
    diagnostics,
    durationSeconds: integer('--duration-seconds', 600, 10, 1800),
    intervalMs: integer('--interval-ms', 2000, 250, 10000),
    units,
    filter: values.get('--filter') ?? null,
    waves: rawWaves === undefined ? (mode === 'smoke' ? 1 : 3) : +rawWaves,
    binary: values.has('--binary') ? resolve(root, values.get('--binary')) : null,
    outputDir: values.has('--output-dir')
      ? resolve(root, values.get('--output-dir'))
      : join(
          root,
          '.cache',
          'backend-concurrency',
          `${new Date().toISOString().replaceAll(':', '-')}-${process.pid}-${randomUUID().slice(0, 8)}`
        )
  }
}

export function baselineEnvironment(options, parent = process.env, root = repositoryRoot) {
  const target = resolve(root, parent.CARGO_TARGET_DIR || defaultTarget)
  const productionTarget = join(root, 'target')
  if (target === productionTarget || target.startsWith(`${productionTarget}${sep}`))
    throw new Error('CARGO_TARGET_DIR must be isolated from the workspace target directory')
  const environment = {
    ...parent,
    CARGO_TARGET_DIR: target,
    CARGO_BUILD_JOBS: '4',
    CARGO_INCREMENTAL: '0',
    CARGO_PROFILE_RELEASE_DEBUG: '0',
    CARGO_PROFILE_TEST_DEBUG: '0',
    CAPTAIN_PERFORMANCE_DIAGNOSTICS: options.diagnostics === 'off' ? '0' : '1',
    BACKEND_BASELINE_OUTPUT: join(options.outputDir, 'cases.jsonl'),
    BACKEND_BASELINE_MODE: options.mode,
    BACKEND_BASELINE_WAVES: String(options.waves),
    BACKEND_BASELINE_DURATION_SECONDS: String(options.durationSeconds),
    BACKEND_BASELINE_INTERVAL_MS: String(options.intervalMs),
    BACKEND_BASELINE_UNITS: String(options.units),
    RUST_TEST_THREADS: '1'
  }
  // A stale parent filter must not silently narrow a requested full run.
  delete environment.BACKEND_BASELINE_FILTER
  if (options.filter !== null) environment.BACKEND_BASELINE_FILTER = options.filter
  return environment
}

export function manifestSourcePath(path) {
  const normalized = path.replaceAll('\\', '/')
  if (
    normalized
      .split('/')
      .some((part) => ['..', '.cache', 'target', 'node_modules', '__screenshots__'].includes(part))
  )
    return false
  if (
    /^(?:Cargo\.(?:toml|lock)|pnpm-lock\.yaml|package\.json|[^/]*config\.(?:json|ts|mjs))$/u.test(
      normalized
    )
  )
    return true
  return (
    /^(?:crates|src|packages|scripts)\//u.test(normalized) &&
    /\.(?:rs|ts|tsx|js|mjs|cjs|json|toml)$/u.test(normalized)
  )
}

export function cargoTestExecutable(record) {
  return record?.reason === 'compiler-artifact' &&
    record.profile?.test === true &&
    record.target?.name === 'core-server' &&
    record.target?.kind?.includes('bin') &&
    typeof record.executable === 'string' &&
    isAbsolute(record.executable)
    ? record.executable
    : null
}

export function parseResourceSample(stdout, withThreads) {
  const values = stdout.trim().split(/\s+/u)
  const count = withThreads ? 2 : 1
  if (values.length !== count || values.some((value) => !/^\d+$/u.test(value))) return null
  return { rss_bytes: Number(values[0]) * 1024, threads: withThreads ? Number(values[1]) : null }
}

export function parseMacThreadCount(stdout, pid) {
  const rows = stdout.trim().split(/\r?\n/u)
  // macOS ps -M prints the PID on the first thread only; following thread rows leave it blank.
  // Inspect only our own benchmark child, retain only the count, never command/user fields.
  if (!/^USER\s+PID\s+.*\bSTAT\b.*\bCOMMAND$/u.test(rows[0] ?? '')) return null
  if (rows[1]?.trim().split(/\s+/u)[1] !== String(pid)) return null
  return rows.slice(1).filter((row) => row.trim()).length
}

function command(program, args, root = repositoryRoot) {
  const result = spawnSync(program, args, {
    cwd: root,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    timeout: 30_000
  })
  if (result.error || result.status !== 0)
    throw new Error(
      `${program} ${args.join(' ')} failed: ${result.error?.message ?? result.stderr?.slice(0, 400)}`
    )
  return result.stdout
}

function digest(value) {
  return createHash('sha256').update(value).digest('hex')
}
async function hashFile(path) {
  const hash = createHash('sha256')
  for await (const chunk of createReadStream(path)) hash.update(chunk)
  return hash.digest('hex')
}
function toolVersion(program, args) {
  try {
    return command(program, args).trim()
  } catch (error) {
    return { unavailable: true, reason: error.message }
  }
}

async function sourceMetadata() {
  const paths = command('git', ['ls-files', '-z', '--cached', '--others', '--exclude-standard'])
    .split('\0')
    .filter(manifestSourcePath)
  const files = []
  for (const path of [...new Set(paths)].sort()) {
    try {
      files.push({ path, sha256: await hashFile(join(repositoryRoot, path)) })
    } catch (error) {
      if (error.code !== 'ENOENT') throw error
      files.push({ path, deleted: true })
    }
  }
  return {
    head: command('git', ['rev-parse', 'HEAD']).trim(),
    tracked_diff_sha256: digest(command('git', ['diff', '--binary', 'HEAD'])),
    source_manifest_sha256: digest(JSON.stringify(files)),
    source_manifest: files,
    lockfiles: files.filter(({ path }) => ['Cargo.lock', 'pnpm-lock.yaml'].includes(path)),
    note: 'Source manifest includes tracked and untracked source/config files; excludes caches, screenshots and output artifacts. Diff contents are never persisted.'
  }
}

function metadataEnvironment(environment) {
  const keys = [
    'CARGO_TARGET_DIR',
    'CARGO_BUILD_JOBS',
    'CARGO_INCREMENTAL',
    'CARGO_PROFILE_RELEASE_DEBUG',
    'CARGO_PROFILE_TEST_DEBUG',
    'CAPTAIN_PERFORMANCE_DIAGNOSTICS',
    'BACKEND_BASELINE_OUTPUT',
    'BACKEND_BASELINE_MODE',
    'BACKEND_BASELINE_WAVES',
    'BACKEND_BASELINE_FILTER',
    'BACKEND_BASELINE_DURATION_SECONDS',
    'BACKEND_BASELINE_INTERVAL_MS',
    'BACKEND_BASELINE_UNITS',
    'RUST_TEST_THREADS'
  ]
  return Object.fromEntries(
    keys.filter((key) => environment[key] !== undefined).map((key) => [key, environment[key]])
  )
}

function samplingCapability() {
  const result = spawnSync('ps', ['-p', String(process.pid), '-o', 'rss=', '-o', 'nlwp='], {
    encoding: 'utf8',
    timeout: 2_000
  })
  const withThreads = result.status === 0 && parseResourceSample(result.stdout, true) !== null
  const macProbe =
    !withThreads && process.platform === 'darwin'
      ? spawnSync('ps', ['-M', '-p', String(process.pid)], { encoding: 'utf8', timeout: 2_000 })
      : null
  const macThreads =
    macProbe?.status === 0 && parseMacThreadCount(macProbe.stdout, process.pid) !== null
  return {
    withThreads,
    macThreads,
    interval_ms: 1_000,
    scope: 'test executable only; excludes runner/cargo and subprocesses',
    thread_count: withThreads
      ? 'ps nlwp'
      : macThreads
        ? 'macOS ps -M thread rows; raw user/command fields discarded'
        : 'unavailable: no supported thread-count sampler',
    rss: 'ps rss KiB converted to bytes; low-frequency samples, not an exact peak'
  }
}

async function runLogged(program, args, environment, outputDir, label, sampleResources = false) {
  const stdout = openSync(join(outputDir, `${label}.stdout.log`), 'wx')
  let stderr
  try {
    stderr = openSync(join(outputDir, `${label}.stderr.log`), 'wx')
  } catch (error) {
    closeSync(stdout)
    throw error
  }
  let samples
  let timer
  const started = process.hrtime.bigint()
  const capability = sampleResources ? samplingCapability() : null
  const child = spawn(program, args, {
    cwd: repositoryRoot,
    env: environment,
    stdio: ['ignore', stdout, stderr]
  })
  closeSync(stdout)
  closeSync(stderr)
  let samplingError = null
  let spawnError = null
  const forward = (signal) => {
    if (child.exitCode === null) child.kill(signal)
  }
  const onInterrupt = () => forward('SIGINT')
  const onTerminate = () => forward('SIGTERM')
  process.on('SIGINT', onInterrupt)
  process.on('SIGTERM', onTerminate)
  const completion = new Promise((resolveCompletion) => {
    child.on('error', (error) => {
      spawnError = error.message
    })
    child.once('close', (code, signal) =>
      resolveCompletion({ code, signal, spawn_error: spawnError })
    )
  })
  try {
    if (sampleResources && child.pid) {
      samples = openSync(join(outputDir, 'resources.jsonl'), 'wx')
      const sample = () => {
        const args = ['-p', String(child.pid), '-o', 'rss=']
        if (capability.withThreads) args.push('-o', 'nlwp=')
        const result = spawnSync('ps', args, { encoding: 'utf8', timeout: 2_000 })
        const measured =
          result.status === 0 ? parseResourceSample(result.stdout, capability.withThreads) : null
        if (measured && capability.macThreads) {
          const threads = spawnSync('ps', ['-M', '-p', String(child.pid)], {
            encoding: 'utf8',
            timeout: 2_000
          })
          measured.threads =
            threads.status === 0 ? parseMacThreadCount(threads.stdout, child.pid) : null
        }
        const record = {
          elapsed_ms: Number(process.hrtime.bigint() - started) / 1e6,
          ...(measured ?? { rss_bytes: null, threads: null, unavailable: true })
        }
        try {
          writeSync(samples, `${JSON.stringify(record)}\n`)
        } catch (error) {
          samplingError = error.message
          clearInterval(timer)
        }
      }
      sample()
      timer = setInterval(sample, capability.interval_ms)
    }
    const outcome = await completion
    return {
      ...outcome,
      duration_ms: Number(process.hrtime.bigint() - started) / 1e6,
      resource_sampling: capability,
      sampling_error: samplingError
    }
  } catch (error) {
    // Stop only the child created by this runner; preserve logs and wait for descriptor closure.
    child.kill('SIGTERM')
    await completion
    throw error
  } finally {
    clearInterval(timer)
    if (samples !== undefined) closeSync(samples)
    process.off('SIGINT', onInterrupt)
    process.off('SIGTERM', onTerminate)
  }
}

async function builtExecutable(log) {
  const candidates = new Set()
  for await (const line of createInterface({ input: createReadStream(log), crlfDelay: Infinity })) {
    let record
    try {
      record = JSON.parse(line)
    } catch {
      continue
    }
    const candidate = cargoTestExecutable(record)
    if (candidate) candidates.add(candidate)
  }
  if (candidates.size !== 1)
    throw new Error(`Expected one core-server test executable, found ${candidates.size}`)
  return [...candidates][0]
}

/** Independent postconditions: an empty recovery result must never count as a successful run. */
export function validateBaselineCase(value) {
  const errors = []
  const count = (candidate) => Number.isSafeInteger(candidate) && candidate >= 0
  const accepted = value?.root_starts_durably_accepted
  const recovered = value?.recovered_root_turns
  if (!count(accepted) || accepted === 0) errors.push('invalid_accepted_root_count')
  if (!count(recovered) || recovered !== accepted) errors.push('recovered_root_count_mismatch')
  if (typeof value?.control_rejections_observed !== 'boolean')
    errors.push('missing_control_rejection_state')

  // Rust ConversationTurnTraceTerminalStatus uses Completed / Cancelled (Debug format).
  const terminals = value?.recovery?.root_terminal_statuses
  const entries =
    terminals && typeof terminals === 'object' && !Array.isArray(terminals)
      ? Object.entries(terminals)
      : []
  if (
    !terminals ||
    entries.some(
      ([status, amount]) => !['Completed', 'Cancelled'].includes(status) || !count(amount)
    )
  ) {
    errors.push('invalid_root_terminal_distribution')
  }
  const terminalTotal = entries.reduce((sum, [, amount]) => sum + (count(amount) ? amount : 0), 0)
  if (terminalTotal !== accepted) errors.push('root_terminal_count_mismatch')
  if (value?.control_rejections_observed === false && value?.case?.work !== 'mixed') {
    const expected = value?.case?.work === 'stop' ? 'Cancelled' : 'Completed'
    if ((terminals?.[expected] ?? 0) !== accepted) errors.push('unexpected_root_terminal_status')
  }

  if (value?.format_version >= 2 && (!value?.raw?.rpcs || typeof value.raw.rpcs !== 'object'))
    errors.push('missing_raw_rpcs')
  if (value?.raw?.rpcs) {
    const records = Object.values(value.raw.rpcs)
    if (records.some((record) => !record.response || record.responded_us == null))
      errors.push('missing_rpc_response')
    for (const [operation, metrics] of Object.entries(value.metrics ?? {})) {
      const matching = records.filter((record) => record.operation === operation)
      const successful = matching.filter((record) => record.response?.result !== undefined)
      if (metrics.offered !== matching.length || metrics.success_response !== successful.length)
        errors.push(`rpc_count_mismatch:${operation}`)
    }
    if ((value.unexpected_rpc_errors?.length ?? 0) > 0) errors.push('unexpected_rpc_errors')
    if ((value.raw.errors?.length ?? 0) > 0) errors.push('runtime_errors')
    if (value.shutdown_timed_out === true) errors.push('shutdown_timeout')
    if (value.recovery?.post_reopen_in_progress !== 0) errors.push('recovery_unfinished')
    if (value.format_version >= 2) {
      const starts = records.filter((record) => record.operation === 'agent.startConversationTurn')
      if (starts.length !== value.case.roots * value.case.waves)
        errors.push('planned_start_count_mismatch')
      if (starts.filter((record) => record.response?.result?.runId).length !== accepted)
        errors.push('accepted_start_count_mismatch')
      const operations = new Set(records.map((record) => record.operation))
      for (const operation of operations) {
        const matching = records.filter((record) => record.operation === operation)
        const metrics = value.metrics?.[operation]
        if (!metrics) {
          errors.push(`missing_operation:${operation}`)
          continue
        }
        if (
          metrics.dispatch_admitted !==
            matching.filter((record) => record.dispatch_admitted).length ||
          metrics.rejected_or_error !==
            matching.filter((record) => record.response?.error !== undefined).length ||
          metrics.no_response !== matching.filter((record) => !record.response).length
        )
          errors.push(`outcome_count_mismatch:${operation}`)
      }
      if (
        records.some(
          (record) =>
            record.operation === 'storage.loadConversation' &&
            !record.response?.error &&
            (record.response?.result?.id !== `baseline-${record.owner}` ||
              !Number.isSafeInteger(record.response?.result?.messages))
        )
      )
        errors.push('invalid_background_read')
    }
    if (value.case?.work === 'mixed') {
      const expectedReads =
        Math.floor(((value.case.waves - 1) * value.fixed_wave_interval_ms) / 500) + 1
      if (
        records.filter((record) => record.operation === 'storage.loadConversation').length !==
        expectedReads
      )
        errors.push('planned_background_read_count_mismatch')
      const acceptedWaves = records.filter(
        (record) =>
          record.operation === 'agent.startConversationTurn' && record.response?.result?.runId
      )
      for (const [waveKind, operation] of [
        [3, 'agent.approveAction'],
        [4, 'agent.cancelRun']
      ]) {
        const expected = acceptedWaves.filter(
          (record) => Number(record.provider_key?.split('-wave-')[1]) % 5 === waveKind
        ).length
        const offered = records.filter((record) => record.operation === operation).length
        if (offered !== expected) errors.push(`mixed_missing_controls:${operation}`)
      }
      const stopped = records.filter(
        (record) => record.operation === 'agent.cancelRun' && record.response?.result !== undefined
      ).length
      if (value.control_rejections_observed === false && (terminals?.Cancelled ?? 0) !== stopped)
        errors.push('mixed_cancel_count_mismatch')
    }
  }
  if (value?.case?.work === 'tree') {
    const units = value.case.units
    const roots = value.case.roots
    // The fixture deliberately retains the production 50-Turn cap, roots included.
    if (!count(units) || !count(roots) || roots === 0 || roots > units || units > 50) {
      errors.push('invalid_tree_population')
    } else {
      const expected = new Set()
      for (let root = 0; root < roots; root += 1) {
        const children =
          Math.floor((units - roots) / roots) + Number(root < (units - roots) % roots)
        for (let child = 0; child < children; child += 1)
          expected.add(`root-${root}-child-${child}`)
      }
      const actual = Object.keys(value?.raw?.provider_first_us ?? {}).filter((marker) =>
        /^root-(0|[1-9]\d*)-child-(0|[1-9]\d*)$/u.test(marker)
      )
      if (actual.length !== units - roots || actual.some((marker) => !expected.has(marker)))
        errors.push('child_provider_marker_mismatch')
      // This proves each synthetic child reached the provider, not its durable terminal state.
    }
  }
  return errors
}

export async function inspectResults(path) {
  let cases = 0
  let invalidLines = 0
  const integrityFailures = []
  try {
    for await (const line of createInterface({
      input: createReadStream(path),
      crlfDelay: Infinity
    })) {
      try {
        const value = JSON.parse(line)
        if (value.kind === 'backend_concurrency_case') {
          cases += 1
          const errors = validateBaselineCase(value)
          if (errors.length) integrityFailures.push({ case: value.name ?? null, errors })
        } else invalidLines += 1
      } catch {
        invalidLines += 1
      }
    }
  } catch (error) {
    if (error.code !== 'ENOENT') throw error
    return { cases, invalidLines, integrityFailures, missing: true }
  }
  return { cases, invalidLines, integrityFailures, missing: false }
}

export async function main(args = process.argv.slice(2)) {
  const options = parseOptions(args)
  const environment = baselineEnvironment(options)
  await mkdir(dirname(options.outputDir), { recursive: true })
  // Reject even an empty existing directory: no previous run or user file is overwritten.
  await mkdir(options.outputDir)
  console.log(`Backend baseline results: ${options.outputDir}`)
  const metadata = {
    format_version: 1,
    started_at: new Date().toISOString(),
    status: 'initializing',
    options,
    environment: metadataEnvironment(environment),
    inherited_build_flags_sha256: digest(
      JSON.stringify({
        rustflags: environment.RUSTFLAGS ?? '',
        encoded: environment.CARGO_ENCODED_RUSTFLAGS ?? '',
        rustup_toolchain: environment.RUSTUP_TOOLCHAIN ?? ''
      })
    ),
    machine: {
      platform: os.platform(),
      release: os.release(),
      architecture: os.arch(),
      cpu_model: os.cpus()[0]?.model ?? null,
      logical_cpus: os.cpus().length,
      total_ram_bytes: os.totalmem()
    },
    tools: {
      node: process.version,
      cargo: toolVersion('cargo', ['--version']),
      rustc: toolVersion('rustc', ['--version', '--verbose']),
      pnpm: toolVersion('pnpm', ['--version'])
    },
    limitations: [
      'Core intake baseline excludes Electron/Renderer click latency.',
      'Full defaults to three burst waves; its dedicated steady-text case is 60 seconds, not the final soak gate.',
      'Small-sample p99 is not reliable; inspect per-case sample counts.',
      'Postconditions verify accepted/recovered root counts and terminal distribution; Tree child provider coverage does not prove every child durable terminal.',
      'External resource sampling excludes child tool processes; unavailable thread counts remain null.',
      'Provided --binary is hashed but its correspondence to current source must be verified by the caller.'
    ]
  }
  try {
    metadata.source = await sourceMetadata()
    await writeFile(
      join(options.outputDir, 'metadata.json'),
      `${JSON.stringify(metadata, null, 2)}\n`,
      { flag: 'wx' }
    )
    let binary = options.binary
    if (!binary) {
      const cargoArgs = [
        'test',
        '--locked',
        '--release',
        '-p',
        'mycopilot-core-server',
        '--bin',
        'core-server',
        '--no-run',
        '--message-format=json'
      ]
      metadata.build = {
        command: ['cargo', ...cargoArgs],
        ...(await runLogged('cargo', cargoArgs, environment, options.outputDir, 'build'))
      }
      if (metadata.build.code !== 0)
        throw new Error(
          `Build failed (${metadata.build.code ?? metadata.build.signal}); logs retained`
        )
      binary = await builtExecutable(join(options.outputDir, 'build.stdout.log'))
    }
    const binaryStat = await stat(binary)
    if (!binaryStat.isFile()) throw new Error('--binary must identify a regular test executable')
    metadata.binary = {
      path: binary,
      sha256: await hashFile(binary),
      size_bytes: binaryStat.size,
      source: options.binary ? 'provided' : 'built_by_runner'
    }
    const testArgs = [TEST_NAME, '--exact', '--ignored', '--nocapture', '--test-threads=1']
    metadata.run = {
      command: [binary, ...testArgs],
      ...(await runLogged(binary, testArgs, environment, options.outputDir, 'test', true))
    }
    const after = await sourceMetadata()
    metadata.source_after = {
      head: after.head,
      tracked_diff_sha256: after.tracked_diff_sha256,
      source_manifest_sha256: after.source_manifest_sha256
    }
    metadata.source_changed_during_run =
      metadata.source.head !== after.head ||
      metadata.source.tracked_diff_sha256 !== after.tracked_diff_sha256 ||
      metadata.source.source_manifest_sha256 !== after.source_manifest_sha256
    metadata.results = await inspectResults(environment.BACKEND_BASELINE_OUTPUT)
    const valid =
      metadata.run.code === 0 &&
      metadata.results.cases > 0 &&
      metadata.results.invalidLines === 0 &&
      metadata.results.integrityFailures.length === 0 &&
      !metadata.results.missing &&
      !metadata.run.sampling_error &&
      !metadata.source_changed_during_run
    metadata.status = valid ? 'completed' : 'failed'
    if (!valid) process.exitCode = metadata.run.code || 1
  } catch (error) {
    metadata.status = 'failed'
    metadata.error = error.message
    process.exitCode = metadata.build?.code || 1
  } finally {
    metadata.finished_at = new Date().toISOString()
    await writeFile(
      join(options.outputDir, 'metadata.json'),
      `${JSON.stringify(metadata, null, 2)}\n`
    )
    console.log(`Backend baseline ${metadata.status}; see metadata.json and retained logs.`)
  }
  return metadata
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error) => {
    console.error(error.message)
    process.exitCode = 1
  })
}
