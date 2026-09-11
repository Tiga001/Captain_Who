import { execFileSync } from 'node:child_process'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { createServer as createHttpServer, type ServerResponse } from 'node:http'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { chromium, type Browser, type Page } from 'playwright'
import { createServer, type ViteDevServer } from 'vite'
import react from '@vitejs/plugin-react'
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import type { TrustedIpcMain } from '../ipc/trustedIpc'

const windows = vi.hoisted(() => ({ pages: [] as Page[] }))
vi.mock('electron', () => ({
  app: { isPackaged: true },
  BrowserWindow: {
    getAllWindows: () =>
      windows.pages.map((page) => ({
        isDestroyed: () => page.isClosed(),
        webContents: {
          isDestroyed: () => page.isClosed(),
          send: (channel: string, value: unknown) => {
            void page
              .evaluate(
                ({ channel, value }) =>
                  window.dispatchEvent(new CustomEvent(channel, { detail: value })),
                { channel, value }
              )
              .catch(() => undefined)
          }
        }
      }))
  }
}))
import { registerAgentIpc } from '../ipc/agentIpc'
import { registerStorageIpc } from '../ipc/storageIpc'
import { CoreServer } from './coreServer'

const root = resolve(__dirname, '../../..')
const originalResources = Object.getOwnPropertyDescriptor(process, 'resourcesPath')
let dataRoot = ''
let core: CoreServer
let vite: ViteDevServer
let browser: Browser
const handlers = new Map<string, (...args: unknown[]) => unknown>()
const invocations: Array<{
  channel: string
  input: unknown
  result: unknown
  startedAt: number
  finishedAt: number
}> = []
const requests: Array<{
  body: { messages: Array<{ role: string; content: unknown }> }
  response: ServerResponse
}> = []
const provider = createHttpServer(async (request, response) => {
  const chunks: Buffer[] = []
  for await (const chunk of request) chunks.push(Buffer.from(chunk))
  requests.push({ body: JSON.parse(Buffer.concat(chunks).toString()), response })
})
// Production user input may have a timing prefix; backend-observed role=user records are separate.
function latestQueuedInput(index: number): string | undefined {
  return requests[index].body.messages
    .filter((message) => message.role === 'user')
    .flatMap((message) => {
      if (typeof message.content === 'string') return [message.content]
      if (!Array.isArray(message.content)) return []
      return message.content.flatMap((part: { text?: unknown }) =>
        typeof part.text === 'string' ? [part.text] : []
      )
    })
    .map((content) => /(?:^|\n)(排队消息[123])$/.exec(content)?.[1])
    .filter((content) => content !== undefined)
    .at(-1)
}

async function waitForRequestCount(page: Page, count: number): Promise<void> {
  try {
    await expect.poll(() => requests.length, { timeout: 5000 }).toBe(count)
  } catch (error) {
    throw new Error(
      JSON.stringify({
        expectedRequests: count,
        actualRequests: requests.length,
        messages: JSON.parse((await page.getByTestId('messages').textContent()) ?? '[]').map(
          (message: {
            id: string
            role: string
            content: string
            status: string
            agentRun?: { runId: string; status: string }
          }) => ({
            id: message.id,
            role: message.role,
            content: message.content,
            status: message.status,
            runId: message.agentRun?.runId,
            runStatus: message.agentRun?.status
          })
        ),
        notices: JSON.parse((await page.getByTestId('notices').textContent()) ?? '[]'),
        autoSend: await page.getByTestId('auto-send').textContent(),
        invocations: invocations.map((entry) => ({
          channel: entry.channel,
          startedAt: entry.startedAt,
          finishedAt: entry.finishedAt,
          ...([
            HOST_CHANNELS.agent.preflightProviderTransition,
            HOST_CHANNELS.agent.startProviderTransition,
            HOST_CHANNELS.agent.startConversationTurn
          ].includes(entry.channel as never)
            ? { input: entry.input, result: entry.result }
            : {})
        }))
      }),
      { cause: error }
    )
  }
}

function completeReply(index: number): void {
  const chunks = [
    {
      choices: [
        { delta: { role: 'assistant', content: `回复${index}完成。` }, finish_reason: null }
      ]
    },
    { choices: [{ delta: {}, finish_reason: 'stop' }] },
    { choices: [], usage: { prompt_tokens: 10, completion_tokens: 2, total_tokens: 12 } }
  ]
  requests[index].response.writeHead(200, { 'Content-Type': 'text/event-stream' })
  requests[index].response.end(
    chunks.map((chunk) => `data: ${JSON.stringify(chunk)}\n\n`).join('') + 'data: [DONE]\n\n'
  )
}

describe('Queue auto-send Chromium → production submission/lifecycle/IPC → real Core', () => {
  beforeAll(async () => {
    execFileSync(
      'cargo',
      ['build', '--locked', '-p', 'mycopilot-core-server', '--bin', 'core-server'],
      { cwd: root, stdio: 'inherit' }
    )
    Object.defineProperty(process, 'resourcesPath', {
      configurable: true,
      value: join(root, 'target/debug')
    })
    dataRoot = await mkdtemp(join(tmpdir(), 'queue-auto-send-real-core-'))
    await new Promise<void>((resolve) => provider.listen(0, '127.0.0.1', resolve))
    const address = provider.address() as { port: number }
    core = new CoreServer({ appDataRoot: dataRoot })
    await core.saveModelSettings({
      expectedRevision: null,
      apiUrl: `http://127.0.0.1:${address.port}/v1/chat/completions`,
      apiTokenMutation: { type: 'replace', value: 'queue-fixture-token' },
      searchMode: 'disabled',
      tavilyApiKeyMutation: { type: 'keep' },
      models: [
        {
          id: null,
          providerModelId: 'queue-fixture',
          displayName: 'Queue fixture',
          apiUrlOverride: null,
          apiTokenOverrideMutation: { type: 'keep' },
          supportsImage: false,
          contextWindowTokens: 128000,
          providerProfileUpdate: { kind: 'select_generic' },
          inputPrice: '0',
          cachedInputPrice: '',
          outputPrice: '0',
          enabled: true
        }
      ]
    })
    const ipc = {
      handle: (channel: string, handler: (...args: unknown[]) => unknown) =>
        handlers.set(channel, handler)
    } as unknown as TrustedIpcMain
    registerAgentIpc(ipc, core)
    const unavailable = async (): Promise<never> => {
      throw new Error('Native platform action is outside this fixture')
    }
    registerStorageIpc(ipc, core, {
      loadImageFile: unavailable,
      pickProjectFolder: unavailable,
      revealProjectFile: unavailable,
      selectProfileAvatar: unavailable,
      showProjectInFolder: unavailable
    })
    vite = await createServer({
      configFile: false,
      root,
      cacheDir: join(dataRoot, 'vite-cache'),
      plugins: [react()],
      server: { host: '127.0.0.1', port: 0, hmr: false, watch: null },
      resolve: { alias: { '@renderer': join(root, 'src/renderer/src') } }
    })
    await vite.listen()
    const manifest = JSON.parse(
      await readFile(join(root, 'resources/office-renderer-manifest.json'), 'utf8')
    )
    browser = await chromium.launch({
      headless: true,
      executablePath: join(
        root,
        '.cache/office-renderer/current',
        manifest.targets[`${process.platform}-${process.arch}`].executable
      )
    })
  }, 600000)

  afterAll(async () => {
    await browser?.close()
    await vite?.close()
    await core?.shutdown()
    provider.closeAllConnections()
    await new Promise<void>((resolve) => provider.close(() => resolve()))
    if (originalResources) Object.defineProperty(process, 'resourcesPath', originalResources)
    else Reflect.deleteProperty(process, 'resourcesPath')
    if (dataRoot) await rm(dataRoot, { recursive: true, force: true })
  })

  it('drains three queued turns after completion and wakes again for newly appended messages without a second toggle', async () => {
    const page = await browser.newPage()
    windows.pages.push(page)
    const pageErrors: string[] = []
    page.on('pageerror', (error) => pageErrors.push(error.message))
    await page.exposeFunction('__queueInvoke', async (channel: string, input: unknown) => {
      const handler = handlers.get(channel)
      if (!handler) throw new Error(`Unregistered IPC: ${channel}`)
      const startedAt = Date.now()
      const result = await handler({}, input)
      invocations.push({ channel, input, result, startedAt, finishedAt: Date.now() })
      return result
    })
    await page.goto(
      `${vite.resolvedUrls!.local[0]}src/renderer/src/app/__fixtures__/queueAutoSendRealCore.html`
    )
    await page.getByRole('button', { name: '开始初始回复' }).click()
    await expect.poll(() => requests.length).toBe(1)
    await page.getByRole('button', { name: '加入三条队列' }).click()
    await page.getByRole('button', { name: '排队消息菜单' }).first().click()
    await page.getByRole('menuitem', { name: '打开队列自动发送' }).click()
    expect(await page.getByTestId('auto-send').textContent()).toBe('true')
    expect(await page.getByTestId('queue-count').textContent()).toBe('3')
    expect(requests).toHaveLength(1)
    completeReply(0)
    for (let index = 1; index <= 3; index += 1) {
      await waitForRequestCount(page, index + 1)
      expect(latestQueuedInput(index)).toBe(`排队消息${index}`)
      completeReply(index)
    }
    await expect
      .poll(
        async () =>
          JSON.parse((await page.getByTestId('messages').textContent()) ?? '[]').filter(
            (message: { agentRun?: { status: string } }) => message.agentRun?.status === 'completed'
          ).length
      )
      .toBe(4)
    expect(await page.getByTestId('queue-count').textContent()).toBe('0')
    expect(await page.getByTestId('auto-send').textContent()).toBe('true')

    // Appending to an already enabled, empty queue must wake dispatch through live draft state.
    await page.getByRole('button', { name: '加入三条队列' }).click()
    for (let index = 4; index <= 6; index += 1) {
      await waitForRequestCount(page, index + 1)
      expect(latestQueuedInput(index)).toBe(`排队消息${index - 3}`)
      completeReply(index)
    }
    const conversationId = await page.getByTestId('workspace').getAttribute('data-conversation-id')
    await expect
      .poll(async () => {
        const conversation = await core.loadConversation(conversationId!)
        return conversation?.messages.filter(
          (message) =>
            message.role === 'assistant' &&
            message.agentRunJson &&
            JSON.parse(message.agentRunJson).status === 'completed'
        ).length
      })
      .toBe(7)
    const stored = await core.loadConversation(conversationId!)
    expect(
      stored?.messages
        .filter((message) => message.role === 'user')
        .map((message) => message.content)
    ).toEqual([
      '初始请求',
      '排队消息1',
      '排队消息2',
      '排队消息3',
      '排队消息1',
      '排队消息2',
      '排队消息3'
    ])
    expect(await page.getByTestId('queue-count').textContent()).toBe('0')
    expect(await page.getByTestId('notices').textContent()).toBe('[]')
    expect(pageErrors).toEqual([])
    const starts = invocations.filter(
      (entry) => entry.channel === HOST_CHANNELS.agent.startConversationTurn
    )
    expect(starts).toHaveLength(7)
    const preflights = invocations.filter(
      (entry) => entry.channel === HOST_CHANNELS.agent.preflightProviderTransition
    )
    expect(preflights).toHaveLength(6)
    expect(
      invocations.filter((entry) => entry.channel === HOST_CHANNELS.agent.startProviderTransition)
    ).toHaveLength(0)
    for (const preflight of preflights) {
      expect(preflight.result).toMatchObject({ ok: true, value: { decision: 'compatible' } })
    }
  }, 30000)
})
