/* eslint-disable @typescript-eslint/explicit-function-return-type -- This is a Node release script. */

import { spawn } from 'node:child_process'
import { homedir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const SCRIPT_DIRECTORY = dirname(fileURLToPath(import.meta.url))
const REPOSITORY_ROOT = resolve(SCRIPT_DIRECTORY, '..')
const ENCODED_FLAG_SEPARATOR = '\u001f'

function uniqueRemapSources(entries) {
  const seen = new Set()
  return entries.filter(([source]) => {
    const normalized = resolve(source)
    if (normalized === '/' || seen.has(normalized)) return false
    seen.add(normalized)
    return true
  })
}

export function createCargoReleaseEnvironment({
  environment = process.env,
  repositoryRoot = REPOSITORY_ROOT,
  userHome = homedir()
} = {}) {
  if (environment.RUSTFLAGS && !environment.CARGO_ENCODED_RUSTFLAGS) {
    throw new Error(
      'Release builds with RUSTFLAGS must provide the equivalent CARGO_ENCODED_RUSTFLAGS so path remapping can be appended safely'
    )
  }

  const cargoHome = resolve(environment.CARGO_HOME || join(userHome, '.cargo'))
  const rustupHome = resolve(environment.RUSTUP_HOME || join(userHome, '.rustup'))
  const inheritedFlags = (environment.CARGO_ENCODED_RUSTFLAGS ?? '')
    .split(ENCODED_FLAG_SEPARATOR)
    .filter(Boolean)
  const remapFlags = uniqueRemapSources([
    [repositoryRoot, 'workspace'],
    [cargoHome, 'cargo-home'],
    [rustupHome, 'rustup-home']
  ]).flatMap(([source, replacement]) => [
    '--remap-path-prefix',
    `${resolve(source)}=${replacement}`
  ])

  return {
    ...environment,
    CARGO_ENCODED_RUSTFLAGS: [...inheritedFlags, ...remapFlags].join(ENCODED_FLAG_SEPARATOR)
  }
}

async function main() {
  const child = spawn(
    'cargo',
    ['build', '--locked', '--release', '-p', 'mycopilot-core-server', '--bin', 'core-server'],
    {
      cwd: REPOSITORY_ROOT,
      env: createCargoReleaseEnvironment(),
      stdio: 'inherit'
    }
  )

  const exitCode = await new Promise((resolveExit, reject) => {
    child.once('error', reject)
    child.once('exit', (code, signal) => {
      if (signal) reject(new Error(`cargo build terminated by signal ${signal}`))
      else resolveExit(code ?? 1)
    })
  })
  if (exitCode !== 0) process.exitCode = exitCode
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  await main()
}
