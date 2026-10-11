from pathlib import Path
p=Path('scripts/run-backend-concurrency-baseline.mjs');s=p.read_text()
s=s.replace("'--binary'])", "'--binary', '--duration-seconds', '--units', '--interval-ms'])",1)
s=s.replace("if (!['smoke', 'full'].includes(mode)) throw new Error('--mode must be smoke or full')", "if (!['smoke', 'full', 'soak'].includes(mode)) throw new Error('--mode must be smoke, full or soak')",1)
s=s.replace('  return {\n    mode,', '''  const integer = (key, fallback, minimum, maximum) => {
    const raw = values.get(key)
    if (raw !== undefined && (!/^\\d+$/u.test(raw) || +raw < minimum || +raw > maximum))
      throw new Error(`${key} must be an integer from ${minimum} to ${maximum}`)
    if (raw !== undefined && mode !== 'soak') throw new Error(`${key} requires soak mode`)
    return raw === undefined ? fallback : +raw
  }
  const units = integer('--units', 20, 1, 50)
  if (![1, 5, 10, 20, 50].includes(units)) throw new Error('--units must be 1/5/10/20/50')
  return {
    mode,
    durationSeconds: integer('--duration-seconds', 600, 10, 1800),
    intervalMs: integer('--interval-ms', 2000, 250, 10000),
    units,''',1)
s=s.replace("    BACKEND_BASELINE_WAVES: String(options.waves),", "    BACKEND_BASELINE_WAVES: String(options.waves),\n    BACKEND_BASELINE_DURATION_SECONDS: String(options.durationSeconds),\n    BACKEND_BASELINE_INTERVAL_MS: String(options.intervalMs),\n    BACKEND_BASELINE_UNITS: String(options.units),",1)
s=s.replace("    'BACKEND_BASELINE_FILTER',", "    'BACKEND_BASELINE_FILTER',\n    'BACKEND_BASELINE_DURATION_SECONDS',\n    'BACKEND_BASELINE_INTERVAL_MS',\n    'BACKEND_BASELINE_UNITS',",1)
s=s.replace("  if (value?.control_rejections_observed === false) {", "  if (value?.control_rejections_observed === false && value?.case?.work !== 'mixed') {",1)
# Validate real emitted reports strictly, while keeping small version-1 unit fixtures supported.
s=s.replace("  if (value?.case?.work === 'tree') {", '''  if (value?.raw?.rpcs) {
    const records = Object.values(value.raw.rpcs)
    if (records.some((record) => !record.response || record.responded_us == null))
      errors.push('missing_rpc_response')
    for (const [operation, metrics] of Object.entries(value.metrics ?? {})) {
      const matching = records.filter((record) => record.operation === operation)
      const successful = matching.filter((record) => record.response?.result !== undefined)
      if (metrics.offered !== matching.length || metrics.successful !== successful.length)
        errors.push(`rpc_count_mismatch:${operation}`)
    }
    if ((value.unexpected_rpc_errors?.length ?? 0) > 0) errors.push('unexpected_rpc_errors')
    if ((value.raw.errors?.length ?? 0) > 0) errors.push('runtime_errors')
    if (value.shutdown_timed_out === true) errors.push('shutdown_timeout')
    if (value.recovery?.post_reopen_in_progress !== 0) errors.push('recovery_unfinished')
    if (value.case?.work === 'mixed') {
      const stopped = records.filter((record) => record.operation === 'agent.cancelRun' && record.response?.result !== undefined).length
      if (value.control_rejections_observed === false && (terminals?.Cancelled ?? 0) !== stopped)
        errors.push('mixed_cancel_count_mismatch')
    }
  }
  if (value?.case?.work === 'tree') {''',1)
p.write_text(s)
