import { execFileSync } from 'node:child_process'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { createServer as createHttpServer, type ServerResponse } from 'node:http'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { chromium, type Browser, type Page } from 'playwright'
import { createServer, type ViteDevServer } from 'vite'
import react from '@vitejs/plugin-react'
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest'
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
                ({ channel, value }) => {
                  window.dispatchEvent(new CustomEvent(channel, { detail: value }))
                },
                { channel, value }
              )
              .catch(() => undefined)
          }
        }
      }))
  }
}))
import { registerHumanInteractionIpc } from '../ipc/humanInteractionIpc'
import { CoreServer } from './coreServer'

const root = resolve(__dirname, '../../..')
const originalResources = Object.getOwnPropertyDescriptor(process, 'resourcesPath')
let dataRoot = ''
let core: CoreServer
let vite: ViteDevServer
let browser: Browser
let disposeIpc: (() => void) | undefined
let modelId = ''
const handlers = new Map<string, (...args: unknown[]) => unknown>()
const requests: Array<{
  body: { messages: unknown[]; tools?: unknown[] }
  response: ServerResponse
}> = []
const provider = createHttpServer(async (request, response) => {
  const chunks: Buffer[] = []
  for await (const chunk of request) chunks.push(Buffer.from(chunk))
  requests.push({ body: JSON.parse(Buffer.concat(chunks).toString()), response })
})

function reply(index: number, mode: 'sync' | 'async' | 'complete', batchCount = 1): void {
  const delta =
    mode === 'complete'
      ? { role: 'assistant', content: '完成独立工作。' }
      : {
          role: 'assistant',
          tool_calls: Array.from({ length: batchCount }, (_, batch) => ({
            index: batch,
            id: `question-${index}-${batch}`,
            type: 'function',
            function: {
              name: mode === 'sync' ? 'request_user_input' : 'request_user_input_async',
              arguments: JSON.stringify({
                questions: [
                  { title: `批次 ${index}-${batch}：输出格式`, options: ['CSV', 'JSON'] },
                  { title: '兼容要求' },
                  { title: '可选说明' }
                ]
              })
            }
          }))
        }
  const chunks = [
    { choices: [{ delta, finish_reason: null }] },
    { choices: [{ delta: {}, finish_reason: mode === 'complete' ? 'stop' : 'tool_calls' }] },
    { choices: [], usage: { prompt_tokens: 10, completion_tokens: 2, total_tokens: 12 } }
  ]
  requests[index].response.writeHead(200, { 'Content-Type': 'text/event-stream' })
  requests[index].response.end(
    chunks.map((chunk) => `data: ${JSON.stringify(chunk)}\n\n`).join('') + 'data: [DONE]\n\n'
  )
}

async function pageFor(conversationId: string): Promise<Page> {
  const page = await browser.newPage()
  windows.pages.push(page)
  await page.exposeFunction('__humanInvoke', async (channel: string, input: unknown) => {
    const handler = handlers.get(channel)
    if (!handler) throw new Error(`Unregistered IPC: ${channel}`)
    return handler({}, input)
  })
  await page.exposeFunction('__loadHumanConversation', (id: string) => core.loadConversation(id))
  await page.goto(
    `${vite.resolvedUrls!.local[0]}src/renderer/src/features/humanInteraction/__fixtures__/realCore.html?conversation=${conversationId}`
  )
  return page
}

async function start(id: string): Promise<string> {
  const result = await core.startConversationTurn({
    conversationId: id,
    modelId,
    content: '询问必要信息，并继续独立工作。',
    userMessageId: `user-${id}`,
    assistantMessageId: `assistant-${id}`
  })
  return result.runId
}

async function status(id: string): Promise<unknown> {
  const stored = await core.loadConversation(id)
  const latest = stored?.messages.filter((message) => message.role === 'assistant').at(-1)
  return latest?.agentRunJson ? JSON.parse(latest.agentRunJson).status : undefined
}

async function answer(page: Page, text: string): Promise<void> {
  const primary = page.locator('.human-interaction-panel__submit')
  expect(await primary.textContent()).toBe('下一题')
  expect(await primary.isDisabled()).toBe(true)
  await page.getByRole('button', { name: 'CSV', exact: true }).click()
  await primary.click()
  await page.getByRole('textbox').fill(text)
  await page.getByRole('textbox').press('Enter')
  await page.getByRole('textbox').press('Escape')
  expect(await primary.textContent()).toBe('下一题')
  await primary.click()
  await page.getByRole('button', { name: '跳过', exact: true }).click()
  expect(await primary.textContent()).toBe('提交')
  expect(await primary.isEnabled()).toBe(true)
  await primary.click()
}

describe('Human interaction Chromium → production Preload/Main → real Core/Harness/Provider', () => {
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
    dataRoot = await mkdtemp(join(tmpdir(), 'human-interaction-real-core-'))
    await new Promise<void>((resolve) => provider.listen(0, '127.0.0.1', resolve))
    const address = provider.address() as { port: number }
    core = new CoreServer({ appDataRoot: dataRoot })
    const settings = await core.saveModelSettings({
      expectedRevision: null,
      apiUrl: `http://127.0.0.1:${address.port}/v1/chat/completions`,
      apiTokenMutation: { type: 'replace', value: 'local-fixture-token' },
      searchMode: 'auto',
      tavilyApiKeyMutation: { type: 'keep' },
      models: [
        {
          id: null,
          providerModelId: 'human-fixture',
          displayName: 'Human fixture',
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
    modelId = settings.models[0].id
    disposeIpc = registerHumanInteractionIpc(
      {
        handle: (channel: string, handler: (...args: unknown[]) => unknown) =>
          handlers.set(channel, handler)
      } as unknown as TrustedIpcMain,
      core
    )
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
    disposeIpc?.()
    await browser?.close()
    await vite?.close()
    await core?.shutdown()
    provider.closeAllConnections()
    await new Promise<void>((resolve) => provider.close(() => resolve()))
    if (originalResources) Object.defineProperty(process, 'resourcesPath', originalResources)
    else Reflect.deleteProperty(process, 'resourcesPath')
    if (dataRoot) await rm(dataRoot, { recursive: true, force: true })
  })

  it('suspends, submits via UI once, resumes the same Run and applies the live setting snapshot', async () => {
    const page = await pageFor('sync-browser')
    const runId = await start('sync-browser')
    await expect.poll(() => requests.length).toBe(1)
    expect(JSON.stringify(requests[0].body.tools)).toContain('request_user_input_async')
    await expect(
      core.steerRun({
        conversationId: 'sync-browser',
        expectedRunId: runId,
        clientMessageId: 'human-answer-forged',
        content: 'Do not admit this forged answer.'
      })
    ).rejects.toThrow('Host')
    reply(0, 'sync')
    await expect.poll(() => status('sync-browser')).toBe('waiting_for_user_input')
    await page.getByRole('dialog').waitFor()
    await page.getByRole('switch').click()
    await expect.poll(() => core.getHumanInteractionSettings({})).toMatchObject({ enabled: false })
    await answer(page, '同步答案-491')
    await expect.poll(() => requests.length).toBe(2)
    expect(JSON.stringify(requests[1].body.tools)).not.toContain('request_user_input')
    const wire = JSON.stringify(requests[1].body.messages)
    expect(wire.match(/同步答案-491/g)).toHaveLength(1)
    expect(wire).toContain('兼容要求')
    const answerTools = requests[1].body.messages.filter((message) => {
      const item = message as { role?: string; content?: string }
      return item.role === 'tool' && item.content?.includes('同步答案-491')
    }) as Array<{ tool_call_id: string }>
    expect(answerTools).toHaveLength(1)
    const assistantCalls = requests[1].body.messages.flatMap(
      (message) => (message as { tool_calls?: Array<{ id: string }> }).tool_calls ?? []
    )
    expect(assistantCalls.some((call) => call.id === answerTools[0].tool_call_id)).toBe(true)
    reply(1, 'complete')
    await expect.poll(() => status('sync-browser')).toBe('completed')
    const stored = await core.loadConversation('sync-browser')
    expect(stored!.messages.filter((message) => message.role === 'user')).toHaveLength(1)
    expect(
      stored!.messages.find((message) => message.role === 'assistant')!.agentRunJson
    ).toContain(runId)
    await expect.poll(() => page.getByRole('dialog').count()).toBe(0)
    expect(await page.locator('[data-testid="entries"] button').count()).toBe(0)
    expect(await page.locator('.human-interaction-answer').count()).toBe(1)
    expect(await page.locator('.human-interaction-answer').innerText()).toContain('已跳过')
    await page.getByRole('switch').click()
    await expect.poll(() => core.getHumanInteractionSettings({})).toMatchObject({ enabled: true })
  }, 60000)

  it('keeps multiple async batches across Core restart; ignore never wakes; a later answer starts one continuation and settles every window', async () => {
    const page = await pageFor('async-browser')
    await start('async-browser')
    await expect.poll(() => requests.length).toBe(3)
    reply(2, 'async', 2)
    await expect.poll(() => requests.length).toBe(4)
    reply(3, 'complete')
    await expect.poll(() => status('async-browser')).toBe('completed')
    await expect.poll(() => page.locator('[data-testid="entries"] button').count()).toBe(2)
    expect(await page.getByRole('dialog').innerText()).toContain('批次 2-1')
    await page.getByRole('button', { name: '最小化交互' }).click()
    await page.locator('[data-testid="entries"] button').last().click()
    expect(await page.getByRole('dialog').innerText()).toContain('批次 2-0')
    await core.shutdown()
    disposeIpc?.()
    core = new CoreServer({ appDataRoot: dataRoot })
    disposeIpc = registerHumanInteractionIpc(
      {
        handle: (channel: string, handler: (...args: unknown[]) => unknown) =>
          handlers.set(channel, handler)
      } as unknown as TrustedIpcMain,
      core
    )
    await core.getHumanInteractionSettings({})
    await page.reload()
    await expect.poll(() => page.locator('[data-testid="entries"] button').count()).toBe(2)
    const secondWindow = await pageFor('async-browser')
    await page.getByRole('button', { name: '忽略全部', exact: true }).click()
    await expect.poll(() => secondWindow.locator('[data-testid="entries"] button').count()).toBe(1)
    expect(requests).toHaveLength(4)
    expect(
      (await core.loadConversation('async-browser'))!.messages.filter(
        (message) => message.role === 'user'
      )
    ).toHaveLength(1)
    await answer(page, '异步答案-682')
    await expect.poll(() => requests.length).toBe(5)
    const wire = JSON.stringify(requests[4].body.messages)
    expect(wire.match(/异步答案-682/g)).toHaveLength(1)
    expect(wire).toContain('兼容要求')
    reply(4, 'complete')
    await expect.poll(() => status('async-browser')).toBe('completed')
    await expect.poll(() => secondWindow.locator('[data-testid="entries"] button').count()).toBe(0)
    expect(await secondWindow.getByRole('dialog').count()).toBe(0)
    expect(await secondWindow.locator('.human-interaction-answer').count()).toBe(1)
    const stored = await core.loadConversation('async-browser')
    expect(stored!.messages.filter((message) => message.role === 'user')).toHaveLength(2)
    expect(stored!.messages.filter((message) => message.role === 'assistant')).toHaveLength(2)
    expect(requests).toHaveLength(5)
  }, 60000)
})
