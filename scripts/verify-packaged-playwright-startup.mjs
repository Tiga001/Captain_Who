/* eslint-disable @typescript-eslint/explicit-function-return-type */

import { spawn, execFile } from 'node:child_process'
import { constants } from 'node:fs'
import { access, mkdir, mkdtemp, realpath, rm, stat } from 'node:fs/promises'
import { createServer } from 'node:http'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { basename, dirname, join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { promisify } from 'node:util'

import { verifyPackagedManagedPlaywrightMcp } from './verify-packaged-app.mjs'

const execFileAsync = promisify(execFile)
const requireFromBuilder = createRequire(import.meta.resolve('electron-builder/package.json'))
const STARTUP_TIMEOUT_MS = 15_000
const POLL_INTERVAL_MS = 50
const REQUIRED_STABLE_SAMPLES = 3
const MAX_DIAGNOSTIC_BYTES = 256 * 1024
const FIXTURE_CANARY = 'MYCOPILOT_PACKAGED_LOCAL_FIXTURE_V1'

export const PACKAGED_AGENT_MCP_DRIVE_ASSESSMENT = Object.freeze({
  status: 'pending',
  reasonCode: 'no_production_external_managed_mcp_driver',
  conclusion:
    'The unpacked application can be started and its bundled Core/Renderer observed, but the local fixture cannot be driven through Agent managed Playwright from outside the production trust boundary.',
  exactBoundary: [
    'The packaged Main process accepts Agent actions only from its exact trusted Renderer frame over registered Electron IPC.',
    'ManagedPlaywrightBridgeHost accepts only private, typed CoreServer notifications and exposes no CLI, socket, environment hook, or general MCP endpoint.',
    'A fresh offline appData root has no provider turn capable of asking the Agent to invoke the built-in browser capability.'
  ],
  forbiddenShortcuts: [
    'remote_debugging_port_or_pipe',
    'devtools_runtime_injection',
    'unknown_or_test_only_mcp_server',
    'user_profile_or_user_browser_reuse',
    'production_fixture_environment_backdoor'
  ],
  nearestExecutableEvidence:
    'pnpm exec vitest run --project unit src/main/core/managedPlaywrightBridge.electron.test.ts',
  requiredToClose:
    'Exercise the public packaged UI with an offline provider fixture and normal approval UI, without adding a privileged test channel; then record the local managed-browser result.'
})

export function defaultUnpackedMacAppBundle(repositoryRoot = resolve('.')) {
  const outputDirectory = process.arch === 'arm64' ? 'mac-arm64' : 'mac'
  return join(repositoryRoot, 'dist', outputDirectory, 'MyCopilot.app')
}

export function assertNoBackdoorArguments(arguments_) {
  const forbidden = [
    '--remote-debugging-port',
    '--remote-debugging-pipe',
    '--inspect',
    '--inspect-brk',
    '--no-sandbox',
    '--disable-web-security',
    '--allow-running-insecure-content',
    '--load-extension'
  ]
  for (const argument of arguments_) {
    if (forbidden.some((prefix) => argument === prefix || argument.startsWith(`${prefix}=`))) {
      throw new Error(`packaged_playwright_backdoor_argument:${argument.split('=')[0]}`)
    }
  }
}

export function validatePackagedProductionEntry(source) {
  for (const required of ['ManagedPlaywrightBridgeHost', 'isTrustedRendererEvent']) {
    if (!source.includes(required)) {
      throw new Error(`packaged_playwright_production_entry_missing:${required}`)
    }
  }
  for (const forbidden of [
    'MYCOPILOT_PACKAGED_FIXTURE',
    'MYCOPILOT_AGENT_MCP_DRIVER',
    '--managed-playwright-fixture',
    '--remote-debugging-port'
  ]) {
    if (source.includes(forbidden)) {
      throw new Error(`packaged_playwright_production_backdoor:${forbidden}`)
    }
  }
}

export function parseProcessTable(value) {
  return value
    .split('\n')
    .map((line) => line.match(/^\s*(\d+)\s+(\d+)\s+(.+)$/u))
    .filter(Boolean)
    .map((match) => ({ pid: Number(match[1]), ppid: Number(match[2]), command: match[3] }))
}

export function descendantProcesses(processes, rootPid) {
  const descendants = []
  const admitted = new Set([rootPid])
  let changed = true
  while (changed) {
    changed = false
    for (const process of processes) {
      if (admitted.has(process.pid) || !admitted.has(process.ppid)) continue
      admitted.add(process.pid)
      descendants.push(process)
      changed = true
    }
  }
  return descendants
}

export async function runPackagedPlaywrightStartupGate(options = {}) {
  if (process.platform !== 'darwin') {
    throw new Error('packaged_playwright_startup_requires_macos')
  }
  const appBundle = await realpath(
    resolve(options.appBundlePath ?? defaultUnpackedMacAppBundle(options.repositoryRoot))
  )
  if (basename(appBundle) !== 'MyCopilot.app' || !(await stat(appBundle)).isDirectory()) {
    throw new Error('packaged_playwright_invalid_app_bundle')
  }
  const executable = join(appBundle, 'Contents', 'MacOS', 'MyCopilot')
  const resources = join(appBundle, 'Contents', 'Resources')
  const coreServer = join(resources, 'core-server')
  await Promise.all([
    access(executable, constants.X_OK),
    access(coreServer, constants.X_OK),
    access(join(resources, 'app.asar'), constants.R_OK)
  ])
  await verifyPackagedManagedPlaywrightMcp({
    electronPlatformName: 'darwin',
    appOutDir: dirname(appBundle),
    packager: { appInfo: { productFilename: 'MyCopilot' } }
  })
  const asar = requireFromBuilder('@electron/asar')
  validatePackagedProductionEntry(
    asar.extractFile(join(resources, 'app.asar'), 'out/main/index.js').toString('utf8')
  )

  const root = await mkdtemp(join(tmpdir(), 'mycopilot-packaged-playwright-startup-'))
  const userData = join(root, 'user-data')
  const home = join(root, 'home')
  const processTmp = join(root, 'tmp')
  await Promise.all([
    mkdir(userData, { recursive: true, mode: 0o700 }),
    mkdir(home, { recursive: true, mode: 0o700 }),
    mkdir(processTmp, { recursive: true, mode: 0o700 })
  ])
  const canonicalUserData = await realpath(userData)
  const fixture = await createOfflineFixtureProxy()
  let child
  let observedProcesses = []
  let stdout = ''
  let stderr = ''
  try {
    await fixture.selfCheck()
    const arguments_ = [
      `--user-data-dir=${canonicalUserData}`,
      '--disable-background-networking',
      '--disable-component-update',
      '--disable-domain-reliability',
      '--disable-sync',
      `--proxy-server=${fixture.proxyUrl}`,
      '--proxy-bypass-list=',
      '--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1, EXCLUDE localhost'
    ]
    assertNoBackdoorArguments(arguments_)
    child = spawn(executable, arguments_, {
      env: isolatedEnvironment({
        home,
        processTmp,
        proxyUrl: fixture.proxyUrl
      }),
      stdio: ['ignore', 'pipe', 'pipe']
    })
    child.stdout?.setEncoding('utf8')
    child.stderr?.setEncoding('utf8')
    child.stdout?.on('data', (chunk) => {
      stdout = boundedAppend(stdout, chunk)
    })
    child.stderr?.on('data', (chunk) => {
      stderr = boundedAppend(stderr, chunk)
    })

    const startup = await observeStableStartup({
      child,
      coreServer,
      userData: canonicalUserData,
      timeoutMs: options.startupTimeoutMs ?? STARTUP_TIMEOUT_MS,
      requiredStableSamples: options.requiredStableSamples ?? REQUIRED_STABLE_SAMPLES
    })
    observedProcesses = startup.processes
    const appFixtureRequests = fixture.requests.filter((request) => !request.selfCheck)
    return {
      schemaVersion: 1,
      status: 'passed',
      scope: 'unpacked_macos_process_startup_only',
      package: {
        bundle: appBundle,
        sourceFreshness: 'not_established_gate_verifies_the_supplied_bundle_only',
        managedPlaywrightRuntime: 'pinned_and_present',
        coreServer: 'bundled_executable_observed',
        renderer: 'sandboxed_packaged_renderer_observed'
      },
      isolation: {
        userData: 'fresh_temporary_root_removed_after_gate',
        inheritedMyCopilotEnvironment: 'removed',
        userBrowserOrProfile: false,
        remoteDebugging: false,
        unknownMcp: false
      },
      offlineContainment: {
        assurance: 'proxy_and_resolver_containment_not_a_kernel_firewall',
        chromiumProxy: 'loopback_deny_proxy',
        hostResolver: 'non_loopback_names_fail_closed',
        backgroundNetworkingDisabled: true,
        proxiedOutboundAttempts: appFixtureRequests.filter((request) => request.outbound).length
      },
      localFixture: {
        provisioned: true,
        selfCheck: 'passed',
        appRequests: appFixtureRequests.filter((request) => request.fixture).length,
        drivenThroughAgentMcp: false
      },
      productionEntry: {
        trustedRendererIpc: true,
        privateCoreBridge: true,
        externalAgentMcpDriverHook: false
      },
      observed: {
        stableSamples: startup.stableSamples,
        storageDatabase: true,
        coreServerProcesses: startup.coreServerProcesses,
        rendererProcesses: startup.rendererProcesses
      },
      agentMcpE2e: PACKAGED_AGENT_MCP_DRIVE_ASSESSMENT
    }
  } catch (error) {
    const detail = error instanceof Error ? error.message : 'unknown failure'
    throw new Error(
      `packaged_playwright_startup_failed:${detail}\nstdout:\n${stdout}\nstderr:\n${stderr}`
    )
  } finally {
    if (child) await terminatePackagedProcess(child, observedProcesses, appBundle, coreServer)
    await fixture.close()
    await rm(root, { recursive: true, force: true })
  }
}

export async function createOfflineFixtureProxy() {
  const requests = []
  const server = createServer((request, response) => {
    const selfCheck = request.headers['x-mycopilot-fixture-self-check'] === '1'
    const fixture = request.url === '/fixture'
    const outbound = !fixture
    requests.push({
      method: request.method ?? 'GET',
      url: request.url ?? '',
      selfCheck,
      fixture,
      outbound
    })
    if (fixture) {
      response.setHeader('content-type', 'text/html; charset=utf-8')
      response.end(`<!doctype html><title>${FIXTURE_CANARY}</title><h1>${FIXTURE_CANARY}</h1>`)
      return
    }
    response.statusCode = 502
    response.end('offline gate blocked outbound request')
  })
  server.on('connect', (request, socket) => {
    requests.push({
      method: 'CONNECT',
      url: request.url ?? '',
      selfCheck: false,
      fixture: false,
      outbound: true
    })
    socket.end('HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n')
  })
  await new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', resolve)
  })
  const address = server.address()
  if (!address || typeof address === 'string') throw new Error('local_fixture_bind_failed')
  const origin = `http://127.0.0.1:${address.port}`
  return {
    proxyUrl: origin,
    fixtureUrl: `${origin}/fixture`,
    requests,
    selfCheck: async () => {
      const response = await fetch(`${origin}/fixture`, {
        headers: { 'x-mycopilot-fixture-self-check': '1' }
      })
      if (!response.ok || !(await response.text()).includes(FIXTURE_CANARY)) {
        throw new Error('local_fixture_self_check_failed')
      }
    },
    close: async () => {
      server.closeAllConnections()
      await new Promise((resolve) => server.close(resolve))
    }
  }
}

async function observeStableStartup(input) {
  const deadline = Date.now() + boundedInteger(input.timeoutMs, 1_000, 60_000)
  const requiredStableSamples = boundedInteger(input.requiredStableSamples, 1, 10)
  let stableSamples = 0
  let latestProcesses = []
  let latestCore = []
  let latestRenderers = []
  while (Date.now() < deadline) {
    if (input.child.exitCode !== null || input.child.signalCode !== null) {
      throw new Error(`app_exited_before_ready:${input.child.exitCode ?? input.child.signalCode}`)
    }
    const processTable = parseProcessTable(await readProcessTable())
    latestProcesses = descendantProcesses(processTable, input.child.pid)
    latestCore = latestProcesses.filter((process) => process.command === input.coreServer)
    latestRenderers = latestProcesses.filter(
      (process) =>
        process.command.includes('--type=renderer') && process.command.includes('.app/Contents/')
    )
    const storageReady = await exists(join(input.userData, 'storage.sqlite'))
    const lockReady = await exists(join(input.userData, '.storage.sqlite.core-server.lock'))
    if (latestCore.length === 1 && latestRenderers.length >= 1 && storageReady && lockReady) {
      stableSamples += 1
      if (stableSamples >= requiredStableSamples) {
        return {
          processes: latestProcesses,
          coreServerProcesses: latestCore.length,
          rendererProcesses: latestRenderers.length,
          stableSamples
        }
      }
    } else {
      stableSamples = 0
    }
    await delay(POLL_INTERVAL_MS)
  }
  throw new Error(
    `startup_timeout:core=${latestCore.length}:renderer=${latestRenderers.length}:stable=${stableSamples}`
  )
}

function isolatedEnvironment(input) {
  const environment = {}
  for (const [key, value] of Object.entries(process.env)) {
    const upper = key.toUpperCase()
    if (
      upper.startsWith('MYCOPILOT_') ||
      upper.startsWith('ELECTRON_') ||
      upper.startsWith('DYLD_') ||
      upper === 'NODE_OPTIONS' ||
      upper === 'NODE_PATH' ||
      upper.endsWith('_PROXY') ||
      upper === 'NO_PROXY'
    ) {
      continue
    }
    if (value !== undefined) environment[key] = value
  }
  Object.assign(environment, {
    HOME: input.home,
    TMPDIR: input.processTmp,
    HTTP_PROXY: input.proxyUrl,
    HTTPS_PROXY: input.proxyUrl,
    ALL_PROXY: input.proxyUrl,
    NO_PROXY: ''
  })
  return environment
}

async function terminatePackagedProcess(child, observedProcesses, appBundle, coreServer) {
  const admitted = new Map(observedProcesses.map((process) => [process.pid, process]))
  if (child.pid) admitted.set(child.pid, { pid: child.pid, ppid: process.pid, command: appBundle })
  if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM')
  await waitForExit(child, 1_000)
  const current = parseProcessTable(await readProcessTable())
  for (const process of current) {
    const observed = admitted.get(process.pid)
    if (!observed || observed.command !== process.command) continue
    if (process.command === coreServer || process.command.startsWith(`${appBundle}/Contents/`)) {
      safeKill(process.pid, 'SIGKILL')
    }
  }
  if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL')
  await waitForExit(child, 1_000)
}

async function waitForExit(child, timeoutMs) {
  if (child.exitCode !== null || child.signalCode !== null) return
  await Promise.race([new Promise((resolve) => child.once('exit', resolve)), delay(timeoutMs)])
}

function safeKill(pid, signal) {
  try {
    process.kill(pid, signal)
  } catch (error) {
    if (error?.code !== 'ESRCH') throw error
  }
}

async function readProcessTable() {
  const { stdout } = await execFileAsync('/bin/ps', ['-axo', 'pid=,ppid=,command='], {
    maxBuffer: 4 * 1024 * 1024
  })
  return stdout
}

async function exists(path) {
  try {
    await access(path)
    return true
  } catch {
    return false
  }
}

function boundedAppend(current, chunk) {
  return `${current}${String(chunk)}`.slice(-MAX_DIAGNOSTIC_BYTES)
}

function boundedInteger(value, minimum, maximum) {
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
    throw new Error('packaged_playwright_invalid_gate_limit')
  }
  return value
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds))
}

function parseArguments(arguments_) {
  let appBundlePath
  for (let index = 0; index < arguments_.length; index += 1) {
    const argument = arguments_[index]
    if (argument === '--app') {
      appBundlePath = arguments_[index + 1]
      index += 1
      if (!appBundlePath) throw new Error('missing --app value')
      continue
    }
    throw new Error(`unknown argument: ${argument}`)
  }
  return { appBundlePath }
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined
if (invokedPath === import.meta.url) {
  runPackagedPlaywrightStartupGate(parseArguments(process.argv.slice(2)))
    .then((result) => process.stdout.write(`${JSON.stringify(result, null, 2)}\n`))
    .catch((error) => {
      process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`)
      process.exitCode = 1
    })
}
