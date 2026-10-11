import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdirSync } from 'node:fs'
import { createServer, type ServerResponse } from 'node:http'
import { dirname, join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { app, BrowserWindow } from 'electron'
import { CoreServer } from '../coreServer'
import type { CoreJsonRpcClient } from '../jsonRpcClient'
import { RpcDiagnostics } from '../rpcDiagnostics'
import { registerAgentIpc } from '../../ipc/agentIpc'
import { registerStorageIpc } from '../../ipc/storageIpc'
import { createTrustedIpcMain } from '../../ipc/trustedIpc'
import type {
  AcceptanceResult,
  AcceptanceSnapshot,
  AcceptanceWork
} from './backendAcceptance.types'

const [profile, binary, renderer, preload] = process.argv.slice(2)
if (!profile || !binary || !renderer || !preload)
  throw new Error('Explicit isolated profile, prebuilt test binary, renderer and preload required')
mkdirSync(profile, { recursive: true })
app.setPath('userData', profile)
app.setPath('sessionData', join(profile, 'session'))
// Keep the process alive for Core shutdown/reopen verification after the hidden window closes.
app.on('window-all-closed', () => undefined)
const coreData = join(profile, 'core-data')
mkdirSync(coreData, { recursive: true })
const commandDirectory = join(profile, 'synthetic-command')
mkdirSync(commandDirectory, { recursive: true })
const reports: AcceptanceResult[] = []
const terminalEvidence: object[] = []
const rpcCounts: Record<string, number> = {}
const hostMetrics: object[] = []
const rendererDiagnostics: object[] = []
const pageErrors: string[] = []
let offeredActions = 0
let failedActions = 0
let verifiedTerminals = 0
let providerRequests = 0
let verificationReads = 0
let core: CoreServer | undefined
let window: BrowserWindow | undefined
let renewal: ReturnType<typeof setInterval> | undefined
let renewalError: unknown
let fixtureFailed = false
let hostDiagnostics: RpcDiagnostics | undefined
const openResponses = new Set<ServerResponse>()

function textContent(value: unknown): string {
  if (typeof value === 'string') return value
  if (Array.isArray(value)) return value.map(textContent).join('\n')
  if (value && typeof value === 'object' && 'text' in value) return textContent(value.text)
  return ''
}
function frame(response: ServerResponse, value: unknown): void {
  if (!response.destroyed) response.write(`data: ${JSON.stringify(value)}\n\n`)
}
const provider = createServer(async (request, response) => {
  openResponses.add(response)
  response.on('close', () => openResponses.delete(response))
  try {
    const chunks: Buffer[] = []
    for await (const chunk of request) chunks.push(Buffer.from(chunk))
    const body = JSON.parse(Buffer.concat(chunks).toString()) as {
      messages: Array<{ role: string; content: unknown }>
    }
    const markerIndex = body.messages.findLastIndex(
      (message) =>
        message.role === 'user' && textContent(message.content).includes('CAPTAIN_ACCEPTANCE:')
    )
    assert(markerIndex >= 0, 'Every provider request belongs to this synthetic fixture')
    const marker = /CAPTAIN_ACCEPTANCE:(text|approval|stop):(\d+)/.exec(
      textContent(body.messages[markerIndex].content)
    )
    assert(marker)
    providerRequests += 1
    response.writeHead(200, { 'Content-Type': 'text/event-stream' })
    if (marker[1] === 'stop') {
      frame(response, {
        choices: [
          { delta: { role: 'assistant', content: 'Confirmed stop prefix. ' }, finish_reason: null }
        ]
      })
      // Production cancellation closes this response. No external process or long sleep is used.
      return
    }
    if (
      marker[1] === 'approval' &&
      !body.messages.slice(markerIndex + 1).some((message) => message.role === 'tool')
    ) {
      frame(response, {
        choices: [
          {
            delta: {
              role: 'assistant',
              content: 'Preparing a local synthetic command.',
              tool_calls: [
                {
                  index: 0,
                  id: `acceptance-command-${marker[2]}`,
                  type: 'function',
                  function: {
                    name: 'run_command',
                    arguments: JSON.stringify({
                      command: "printf 'acceptance-approved-once\\n'",
                      reason: 'isolated acceptance fixture'
                    })
                  }
                }
              ]
            },
            finish_reason: null
          }
        ]
      })
      frame(response, { choices: [{ delta: {}, finish_reason: 'tool_calls' }] })
    } else {
      for (let index = 0; index < 16 && !response.destroyed; index++) {
        frame(response, {
          choices: [{ delta: { content: `segment-${index} ` }, finish_reason: null }]
        })
        await new Promise((resolve) => setTimeout(resolve, 20))
      }
      frame(response, { choices: [{ delta: {}, finish_reason: 'stop' }] })
      frame(response, {
        choices: [],
        usage: { prompt_tokens: 10, completion_tokens: 16, total_tokens: 26 }
      })
    }
    if (!response.destroyed) response.end('data: [DONE]\n\n')
  } catch (error) {
    pageErrors.push(`provider: ${String(error)}`)
    response.destroy()
  }
})

async function snapshot(): Promise<AcceptanceSnapshot> {
  return window!.webContents.executeJavaScript('window.acceptanceControls.snapshot()')
}
async function waitFor(check: () => Promise<boolean>, label: string): Promise<void> {
  const deadline = performance.now() + 20_000
  while (!(await check())) {
    if (performance.now() >= deadline) throw new Error(`${label} timed out`)
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}
async function invoke(expression: string): Promise<AcceptanceResult> {
  offeredActions += 1
  let result: AcceptanceResult
  try {
    result = (await window!.webContents.executeJavaScript(expression)) as AcceptanceResult
  } catch (error) {
    failedActions += 1
    throw error
  }
  reports.push(result)
  assert.equal(result.accepted, true)
  assert(result.runId && result.conversationId && result.assistantMessageId)
  assert(result.receiptMs >= 0 && result.frameOpportunityMs >= result.receiptMs)
  verificationReads += 1
  const stored = await core!.loadConversation(result.conversationId)
  assert(
    stored?.messages.some((message) => message.id === result.assistantMessageId),
    'An acknowledged start/decision must refer to the exact durable assistant'
  )
  return result
}

async function main(): Promise<void> {
  await app.whenReady()
  await new Promise<void>((resolve) => provider.listen(0, '127.0.0.1', resolve))
  const address = provider.address() as { port: number }
  core = new CoreServer({ appDataRoot: coreData })
  const rpc = Reflect.get(core, 'rpc') as CoreJsonRpcClient
  Reflect.set(rpc, 'resolveCommand', () => ({
    executable: binary,
    args: [
      'transport::bootstrap::acceptance_stdio_server',
      '--exact',
      '--ignored',
      '--nocapture',
      '--test-threads=1'
    ],
    cwd: dirname(binary)
  }))
  const diagnostics = new RpcDiagnostics(true, (summary) => hostMetrics.push(summary))
  hostDiagnostics = diagnostics
  Reflect.set(rpc, 'diagnostics', diagnostics)
  const request = Reflect.get(rpc, 'request') as (...args: unknown[]) => unknown
  Reflect.set(rpc, 'request', (...args: unknown[]) => {
    const method = String(args[0])
    rpcCounts[method] = (rpcCounts[method] ?? 0) + 1
    return Reflect.apply(request, rpc, args)
  })
  await core.saveModelSettings({
    expectedRevision: null,
    apiUrl: `http://127.0.0.1:${address.port}/v1/chat/completions`,
    apiTokenMutation: { type: 'replace', value: 'synthetic-acceptance-only' },
    searchMode: 'disabled',
    tavilyApiKeyMutation: { type: 'keep' },
    models: [
      {
        id: null,
        providerModelId: 'acceptance-fixture',
        displayName: 'Acceptance fixture',
        apiUrlOverride: null,
        apiTokenOverrideMutation: { type: 'keep' },
        supportsImage: false,
        contextWindowTokens: 256000,
        providerProfileUpdate: { kind: 'select_generic' },
        inputPrice: '0',
        cachedInputPrice: '',
        outputPrice: '0',
        enabled: true
      }
    ]
  })
  await core.saveProject({
    id: 'acceptance-project',
    name: 'Synthetic acceptance workspace',
    folders: [
      {
        id: 'acceptance-project-folder',
        path: commandDirectory,
        alias: 'acceptance',
        role: 'primary',
        sortOrder: 0,
        createdAt: 1
      }
    ],
    createdAt: 1,
    pinnedAt: null
  })
  console.log(`CAPTAIN_ACCEPTANCE_CORE_PID=${Reflect.get(rpc, 'child')?.pid}`)
  let revision = 0
  let lease = Promise.resolve()
  const renew = (): Promise<void> => {
    lease = lease.then(async () => {
      const issuedAt = Date.now()
      await core!.setExecutionAccess({
        revision: ++revision,
        identityEpoch: 1,
        reason: 'allowed',
        issuedAt,
        validUntil: issuedAt + 60_000
      })
    })
    return lease
  }
  await renew()
  renewal = setInterval(() => {
    void renew().catch((error) => {
      renewalError = error
    })
  }, 20_000)
  window = new BrowserWindow({
    show: false,
    width: 800,
    height: 600,
    webPreferences: {
      preload,
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      backgroundThrottling: false
    }
  })
  const expectedUrl = pathToFileURL(renderer).href
  const ipc = createTrustedIpcMain((event) =>
    Boolean(
      window &&
      event.sender === window.webContents &&
      event.senderFrame === window.webContents.mainFrame &&
      event.senderFrame?.url === expectedUrl
    )
  )
  registerAgentIpc(ipc, core, renew)
  const unavailable = async (): Promise<never> => {
    throw new Error('Outside synthetic acceptance scope')
  }
  registerStorageIpc(ipc, core, {
    loadImageFile: unavailable,
    pickProjectFolder: unavailable,
    revealProjectFile: unavailable,
    selectProfileAvatar: unavailable,
    showProjectInFolder: unavailable
  })
  window.webContents.on('console-message', (_event, level, message) => {
    if (level >= 3) pageErrors.push(message)
  })
  await window.loadFile(renderer)
  await waitFor(
    () => window!.webContents.executeJavaScript('Boolean(window.acceptanceControls)'),
    'Renderer startup'
  )
  diagnostics.flush()
  hostMetrics.length = 0
  for (const method of Object.keys(rpcCounts)) delete rpcCounts[method]
  for (const work of ['text', 'approval', 'stop'] as AcceptanceWork[]) {
    for (let index = 0; index < 10; index++) {
      const started = await invoke(
        `window.acceptanceControls.submit(${JSON.stringify(work)}, ${index})`
      )
      if (work === 'approval') {
        await waitFor(async () => (await snapshot()).pendingApprovals === 1, 'Approval projection')
        await invoke('window.acceptanceControls.approve()')
      } else if (work === 'stop') {
        await waitFor(
          async () => (await snapshot()).content.includes('Confirmed stop prefix.'),
          'Live stop prefix'
        )
        await invoke('window.acceptanceControls.stop()')
      }
      const expected = work === 'stop' ? 'cancelled' : 'completed'
      await waitFor(async () => {
        const current = await snapshot()
        return (
          current.status === expected &&
          current.renderedStatus === expected &&
          current.renderedContent === current.content
        )
      }, 'Terminal DOM projection')
      verificationReads += 1
      const stored = await core.loadConversation(started.conversationId!)
      const message = stored?.messages.find((message) => message.id === started.assistantMessageId)
      assert(message?.agentRunJson)
      assert.equal(JSON.parse(message.agentRunJson).status, expected)
      assert(message.content.length > 0)
      const expectedText = Array.from({ length: 16 }, (_, index) => `segment-${index} `).join('')
      if (work === 'stop') assert.equal(message.content, 'Confirmed stop prefix. ')
      else assert(message.content.endsWith(expectedText), 'Every generated text segment survives')
      // Done triggers the production durable reconciliation read. A cancelled state can arrive
      // before that async read applies its preserved partial narration, so observe the full join.
      await waitFor(async () => {
        const current = await snapshot()
        return (
          current.assistantMessageId === message.id &&
          current.status === expected &&
          current.renderedStatus === expected &&
          current.content === message.content &&
          current.renderedContent === message.content
        )
      }, 'Durable terminal content applied to DOM')
      if (work === 'approval')
        assert.equal(
          JSON.parse(message.agentRunJson).toolCalls.filter(
            (call: { tool: string }) => call.tool === 'run_command'
          ).length,
          1
        )
      terminalEvidence.push({
        work,
        index,
        runId: started.runId,
        assistantMessageId: message.id,
        status: expected,
        contentBytes: Buffer.byteLength(message.content),
        contentSha256: createHash('sha256').update(message.content).digest('hex')
      })
      verifiedTerminals += 1
      // Flush outside every timed action to retain all cross-boundary traces within their
      // production bounded windows, including the later approval and Stop scenarios.
      rendererDiagnostics.push(
        ...(await window.webContents.executeJavaScript(
          'window.acceptanceControls.flushDiagnostics()'
        ))
      )
      diagnostics.flush()
    }
  }
  rendererDiagnostics.push(
    ...(await window.webContents.executeJavaScript('window.acceptanceControls.flushDiagnostics()'))
  )
  diagnostics.flush()
  assert.equal(renewalError, undefined)
  assert.deepEqual(pageErrors, [])
  const current = await snapshot()
  assert.deepEqual(current.notices, [])
  verificationReads += 1
  const before = await core.loadConversation(current.conversationId!)
  clearInterval(renewal)
  await lease
  window.destroy()
  await core.shutdown()
  // A fresh Core process must reconstruct all settled messages from the same isolated disk.
  core = new CoreServer({ appDataRoot: coreData })
  Reflect.set(Reflect.get(core, 'rpc'), 'resolveCommand', Reflect.get(rpc, 'resolveCommand'))
  const reopened = await core.loadConversation(current.conversationId!)
  console.log(`CAPTAIN_ACCEPTANCE_CORE_PID=${Reflect.get(Reflect.get(core, 'rpc'), 'child')?.pid}`)
  assert.equal(reopened?.messages.length, before?.messages.length)
  assert.equal(reopened?.messages.filter((message) => message.role === 'assistant').length, 30)
  assert.equal(providerRequests, 40)
  assert.deepEqual(
    reopened?.messages.map((message) => [
      message.id,
      message.content,
      message.status,
      message.agentRunJson ? JSON.parse(message.agentRunJson) : null
    ]),
    before?.messages.map((message) => [
      message.id,
      message.content,
      message.status,
      message.agentRunJson ? JSON.parse(message.agentRunJson) : null
    ])
  )
  console.log(
    'CAPTAIN_RENDERER_ACCEPTANCE=' +
      JSON.stringify({
        status: 'completed',
        offeredActions,
        completedActions: reports.length,
        failedActions,
        unfinishedActions: 0,
        verifiedTerminals,
        reopenedAssistantMessages: 30,
        providerRequests,
        verificationReads: { measuredCore: verificationReads, reopenedCore: 1 },
        reports,
        terminalEvidence,
        rpcCounts,
        hostMetrics,
        rendererDiagnostics,
        boundary:
          'Programmatic DOM action handler through production submission/lifecycle/decision/Stop hooks, native Electron preload/IPC and real Core pipe. Renderer monotonic receipt and two-frame opportunity; not hardware input, compositor paint, a saturated workload or a reliable p99. Stop receipt is not terminal durability. Host metrics also include the separately counted fixture verification reads, all outside timed actions.',
        fixture:
          'Release test-support Core full bootstrap, isolated temporary disk, local synthetic model; production guards and business limits unchanged. libtest stdout headers are non-JSON diagnostics ignored by the existing Host parser.'
      })
  )
}

void main()
  .catch(async (error: unknown) => {
    fixtureFailed = true
    const failureSnapshot =
      window && !window.isDestroyed() ? await snapshot().catch(() => null) : null
    if (window && !window.isDestroyed()) {
      rendererDiagnostics.push(
        ...(await window.webContents
          .executeJavaScript('window.acceptanceControls?.flushDiagnostics() ?? []')
          .catch(() => []))
      )
    }
    hostDiagnostics?.flush()
    console.log(
      'CAPTAIN_RENDERER_ACCEPTANCE=' +
        JSON.stringify({
          status: 'failed',
          offeredActions,
          completedActions: reports.length,
          failedActions,
          unfinishedActions: Math.max(0, offeredActions - reports.length - failedActions),
          verifiedTerminals,
          reports,
          hostMetrics,
          rendererDiagnostics,
          pageErrors,
          failureSnapshot,
          error: String(error)
        })
    )
  })
  .finally(async () => {
    clearInterval(renewal)
    if (window && !window.isDestroyed()) window.destroy()
    for (const response of openResponses) response.destroy()
    provider.closeAllConnections()
    await new Promise<void>((resolve) => provider.close(() => resolve()))
    await core?.shutdown().catch(() => undefined)
    app.exit(fixtureFailed ? 1 : 0)
  })
