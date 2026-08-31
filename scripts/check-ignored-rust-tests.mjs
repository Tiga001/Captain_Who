/* eslint-disable @typescript-eslint/explicit-function-return-type -- The checker is runtime-validated JavaScript. */
import { globSync, readFileSync, statSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const defaultRepositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const defaultRegistryPath = join(defaultRepositoryRoot, 'scripts/ignored-rust-tests.json')
const functionPattern = /^\s*(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\b/u

function normalizeRepositoryPath(file) {
  return file.replaceAll('\\', '/')
}

function isRegularFile(repositoryRoot, file) {
  return statSync(join(repositoryRoot, file)).isFile()
}

export function findIgnoredRustTests({ repositoryRoot = defaultRepositoryRoot } = {}) {
  const tests = []

  for (const file of globSync('crates/**/*.rs', { cwd: repositoryRoot, nodir: true }).sort()) {
    if (!isRegularFile(repositoryRoot, file)) continue

    const lines = readFileSync(join(repositoryRoot, file), 'utf8').split(/\r?\n/u)
    let ignoredAt = null
    for (let index = 0; index < lines.length; index += 1) {
      const line = lines[index]
      if (/^\s*#\[ignore(?:\s*=\s*[^\]]+)?\]\s*$/u.test(line)) {
        ignoredAt = index + 1
        continue
      }
      if (ignoredAt === null) continue

      const match = functionPattern.exec(line)
      if (match) {
        tests.push({ file: normalizeRepositoryPath(file), name: match[1], line: ignoredAt })
        ignoredAt = null
        continue
      }
      if (/^\s*(?:#\[[^\]]+\]|\/\/.*|\/\*.*\*\/|$)/u.test(line)) continue
      throw new Error(
        `${normalizeRepositoryPath(file)}:${ignoredAt}: #[ignore] is not followed by a test function`
      )
    }
    if (ignoredAt !== null) {
      throw new Error(
        `${normalizeRepositoryPath(file)}:${ignoredAt}: #[ignore] is not followed by a test function`
      )
    }
  }

  return tests
}

export function readIgnoredRustTestRegistry({ registryPath = defaultRegistryPath } = {}) {
  let parsed
  try {
    parsed = JSON.parse(readFileSync(registryPath, 'utf8'))
  } catch (error) {
    throw new Error(
      `Unable to read ignored Rust test registry at ${registryPath}: ${error.message}`
    )
  }
  if (!parsed || typeof parsed !== 'object' || !Array.isArray(parsed.tests)) {
    throw new Error('Ignored Rust test registry must contain a tests array')
  }
  return parsed.tests
}

function testKey({ file, name }) {
  return `${file}::${name}`
}

function validateRegistryEntry(entry, index) {
  const label = `registry entry ${index + 1}`
  if (!entry || typeof entry !== 'object') return [`${label}: must be an object`]
  const failures = []
  for (const property of ['file', 'name', 'owner', 'reason']) {
    if (typeof entry[property] !== 'string' || entry[property].trim() === '') {
      failures.push(`${label}: ${property} must be a non-empty string`)
    }
  }
  const hasRunner = typeof entry.runner === 'string' && entry.runner.trim() !== ''
  const isManual = entry.status === 'manual'
  if (hasRunner === isManual) {
    failures.push(`${label}: specify exactly one of a non-empty runner or status "manual"`)
  }
  return failures
}

export function analyzeIgnoredRustTests({
  repositoryRoot = defaultRepositoryRoot,
  registryPath = defaultRegistryPath
} = {}) {
  const sourceTests = findIgnoredRustTests({ repositoryRoot })
  const registryTests = readIgnoredRustTestRegistry({ registryPath })
  const failures = registryTests.flatMap(validateRegistryEntry)
  const sourceByKey = new Map()
  const registryByKey = new Map()

  for (const test of sourceTests) {
    const key = testKey(test)
    if (sourceByKey.has(key)) failures.push(`source test duplicated: ${key}`)
    sourceByKey.set(key, test)
  }
  for (const test of registryTests) {
    if (!test || typeof test !== 'object') continue
    const key = testKey(test)
    if (registryByKey.has(key)) failures.push(`registry entry duplicated: ${key}`)
    registryByKey.set(key, test)
  }
  for (const key of sourceByKey.keys()) {
    if (!registryByKey.has(key)) failures.push(`ignored source test is unregistered: ${key}`)
  }
  for (const key of registryByKey.keys()) {
    if (!sourceByKey.has(key))
      failures.push(`registry entry does not match an ignored source test: ${key}`)
  }

  return { failures, registryTests, sourceTests }
}

export function assertIgnoredRustTests(options) {
  const analysis = analyzeIgnoredRustTests(options)
  if (analysis.failures.length > 0) {
    throw new Error(`Ignored Rust test registry check failed:\n${analysis.failures.join('\n')}`)
  }
  return analysis
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : null
if (invokedPath === import.meta.url) {
  try {
    const analysis = assertIgnoredRustTests()
    console.log(`Ignored Rust test registry OK: ${analysis.sourceTests.length} tests`)
  } catch (error) {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  }
}
