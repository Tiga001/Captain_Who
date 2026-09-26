import { afterEach, describe, expect, it, vi } from 'vitest'
import { access, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, dirname, isAbsolute, join } from 'node:path'
import type { BrowserContext } from 'playwright'
import { BrowserDownloadBrokerError } from '../browser/BrowserDownloadBroker'
import { BrowserFileBroker } from '../browser/BrowserFileBroker'
import { type ManagedMcpClient } from './ManagedPlaywrightMcpHost'
import { createManagedPlaywrightHostTestFixture } from './ManagedPlaywrightMcpHost.test-fixtures'

const {
  closeTrackedHosts,
  RISK_CONTEXT,
  PARENT_REQUEST_ID,
  sensitiveContext,
  fakeBrowserContext,
  fakeHost,
  riskLease
} = createManagedPlaywrightHostTestFixture()

afterEach(closeTrackedHosts)

describe('ManagedPlaywrightMcpHost', () => {
  it('keeps pure MIME drop automatic while refusing a sensitive grant on that path', async () => {
    const callTool = vi.fn(async () => ({ content: [{ type: 'text', text: 'dropped' }] }))
    const host = fakeHost({ callTool })
    const args = {
      target: '#fixture-drop-zone',
      data: { 'text/plain': 'hello' },
      call_reason: 'Drop plain fixture text.'
    }
    await expect(host.callTool('browser_drop', args)).resolves.toMatchObject({ isError: false })
    expect(callTool).toHaveBeenCalledWith(
      {
        name: 'browser_drop',
        arguments: { target: '#fixture-drop-zone', data: { 'text/plain': 'hello' } }
      },
      undefined,
      expect.any(Object)
    )

    await expect(
      host.callTool('browser_drop', args, {
        authorizationContext: sensitiveContext('browser_drop', {
          ...args,
          paths: ['browser-file:00000000-0000-4000-8000-000000000000']
        })
      })
    ).rejects.toMatchObject({
      code: 'mcp.builtin_playwright.sensitive_grant_drifted',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    expect(callTool).toHaveBeenCalledOnce()
  })

  it('preserves official file-upload cancellation when paths are omitted', async () => {
    const callTool = vi.fn(async () => ({
      content: [{ type: 'text', text: 'File chooser cancelled.' }],
      isError: false
    }))
    const host = fakeHost({ callTool })
    const argumentsValue = {
      call_reason: 'Cancel the current managed page file chooser.'
    }

    await expect(host.callTool('browser_file_upload', argumentsValue)).resolves.toMatchObject({
      isError: false
    })
    expect(callTool).toHaveBeenCalledWith(
      { name: 'browser_file_upload', arguments: {} },
      undefined,
      expect.any(Object)
    )
  })

  it('uses brokered opaque file handles for upload and iframe drop without accepting raw paths', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-file-test-'))
    const source = join(parent, 'selected.txt')
    await writeFile(source, 'selected-file-canary')
    const fileBroker = new BrowserFileBroker({
      rootDirectory: join(parent, 'browser-automation-files'),
      selectionProvider: { selectFiles: vi.fn(async () => [source]) }
    })
    await fileBroker.initialize()
    const inputElement = {
      setInputFiles: vi.fn<(paths: readonly string[]) => Promise<void>>(async () => undefined),
      evaluate: vi.fn<(callback: unknown) => Promise<void>>(async () => undefined),
      dispose: vi.fn(async () => undefined)
    }
    const targetElement = {
      evaluateHandle: vi.fn<(callback: unknown) => Promise<{ asElement(): typeof inputElement }>>(
        async () => ({ asElement: () => inputElement })
      ),
      evaluate: vi.fn<
        (callback: unknown, argument: unknown) => Promise<{ accepted: boolean; fileCount: number }>
      >(async () => ({ accepted: true, fileCount: 1 })),
      dispose: vi.fn(async () => undefined)
    }
    const locator = { elementHandle: vi.fn(async () => targetElement) }
    const page = { locator: vi.fn(() => locator) }
    const context = {
      ...fakeBrowserContext(),
      pages: vi.fn(() => [page])
    } as unknown as BrowserContext
    const callTool = vi.fn(async ({ name }: { name: string }) => ({
      content: [
        {
          type: 'text' as const,
          text: name === 'browser_snapshot' ? '- region "Drop completed" [ref=e42]' : 'uploaded'
        }
      ],
      isError: false
    }))
    const host = fakeHost({
      callTool: callTool as ManagedMcpClient['callTool'],
      fileBroker,
      getBrowserContext: vi.fn(async () => context)
    })
    try {
      const [uploadReference] = await fileBroker.freezeResolvedForRead({
        owner: {
          runId: RISK_CONTEXT.runId,
          activationId: RISK_CONTEXT.activationId,
          capabilityId: 'browser_automation',
          toolCallId: 'call-file-upload'
        },
        paths: [source]
      })
      const handle = uploadReference.handle
      expect(fileBroker.snapshot().handles).toBe(1)

      const uploadArguments = {
        paths: [handle!],
        call_reason: 'Upload the selected fixture file.'
      }
      await expect(
        host.callTool('browser_file_upload', uploadArguments, {
          authorizationContext: sensitiveContext('browser_file_upload', uploadArguments, {
            callId: 'call-file-upload'
          })
        })
      ).resolves.toMatchObject({ isError: false })
      expect(callTool).toHaveBeenCalledOnce()
      const uploadedPaths =
        (callTool.mock.calls[0]?.[0] as { arguments?: { paths?: string[] } }).arguments?.paths ?? []
      const uploadMeta = (
        callTool.mock.calls[0]?.[0] as { arguments?: { _meta?: { cwd?: string } } }
      ).arguments?._meta
      expect(uploadedPaths).toHaveLength(1)
      expect(uploadedPaths[0]).not.toBe(source)
      expect(uploadedPaths[0]).not.toBe(handle)
      expect(isAbsolute(uploadedPaths[0])).toBe(true)
      expect(uploadMeta).toEqual({ cwd: dirname(dirname(uploadedPaths[0])) })
      expect(basename(uploadedPaths[0])).toBe('selected.txt')
      expect(uploadedPaths[0]).toContain('.file-input-')
      expect(uploadedPaths[0]).not.toContain('browser-automation-files')
      await expect(access(uploadedPaths[0])).resolves.toBeUndefined()
      expect(fileBroker.snapshot().handles).toBe(0)
      expect(fileBroker.snapshot().retained).toMatchObject({ leases: 1, files: 1 })

      const [dropReference] = await fileBroker.freezeResolvedForRead({
        owner: {
          runId: RISK_CONTEXT.runId,
          activationId: RISK_CONTEXT.activationId,
          capabilityId: 'browser_automation',
          toolCallId: 'call-file-drop'
        },
        paths: [source]
      })
      const dropHandle = dropReference.handle
      const dropArguments = {
        target: 'f2e9',
        paths: [dropHandle!],
        call_reason: 'Drop the selected fixture file into the exact iframe target.'
      }
      await expect(
        host.callTool('browser_drop', dropArguments, {
          authorizationContext: sensitiveContext('browser_drop', dropArguments, {
            callId: 'call-file-drop'
          })
        })
      ).resolves.toMatchObject({ isError: false })
      expect(callTool).toHaveBeenCalledTimes(2)
      expect(callTool.mock.calls[1]?.[0]).toMatchObject({
        name: 'browser_snapshot',
        arguments: {}
      })
      expect(page.locator).toHaveBeenCalledWith('aria-ref=f2e9')
      const droppedPaths = inputElement.setInputFiles.mock.calls[0]?.[0] as string[]
      expect(droppedPaths).toHaveLength(1)
      expect(basename(droppedPaths[0])).toBe('selected.txt')
      const dropCallback = String(targetElement.evaluate.mock.calls[0]?.[0])
      expect(
        [...dropCallback.matchAll(/new DragEvent\(["'](dragenter|dragover|drop)["']/gu)].map(
          (match) => match[1]
        )
      ).toEqual(['dragenter', 'dragover', 'drop'])
      expect(dropCallback).toContain('dragover.defaultPrevented')
      expect(fileBroker.snapshot().handles).toBe(0)
      expect(fileBroker.snapshot().retained).toMatchObject({ leases: 2, files: 2 })

      const guessedArguments = {
        paths: [source],
        call_reason: 'Upload a guessed path.'
      }
      await expect(
        host.callTool('browser_file_upload', guessedArguments, {
          authorizationContext: sensitiveContext('browser_file_upload', guessedArguments, {
            callId: 'call-guessed-upload'
          })
        })
      ).rejects.toMatchObject({
        code: 'mcp.builtin_playwright.invalid_arguments',
        dispatchCertainty: 'definitely_not_dispatched'
      })
      expect(callTool).toHaveBeenCalledTimes(2)
      await host.close()
      await expect(access(uploadedPaths[0])).resolves.toBeUndefined()
      await host.releaseRun(RISK_CONTEXT.runId)
      await expect(access(uploadedPaths[0])).rejects.toMatchObject({ code: 'ENOENT' })
      expect(fileBroker.snapshot()).toEqual({
        handles: 0,
        bytes: 0,
        retained: { leases: 0, files: 0, bytes: 0 }
      })
    } finally {
      await fileBroker.shutdown()
      await rm(parent, { recursive: true, force: true })
    }
  })

  it('uses proposal-frozen handles while keeping the original workspace path out of upstream dispatch', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-one-call-upload-test-'))
    const source = join(parent, '浙江大学2026年招生资料汇编.pptx')
    await writeFile(source, 'fixture-presentation')
    const fileBroker = new BrowserFileBroker({
      rootDirectory: join(parent, 'browser-automation-files'),
      selectionProvider: { selectFiles: vi.fn(async () => []) }
    })
    const callId = 'call-one-call-upload'
    const [reference] = await fileBroker.freezeResolvedForRead({
      owner: {
        runId: RISK_CONTEXT.runId,
        activationId: RISK_CONTEXT.activationId,
        capabilityId: 'browser_automation',
        toolCallId: callId
      },
      paths: [source]
    })
    const callTool = vi
      .fn<ManagedMcpClient['callTool']>()
      .mockResolvedValue({ content: [{ type: 'text', text: 'uploaded' }], isError: false })
    const host = fakeHost({
      callTool,
      fileBroker,
      preparedFileHandles: [reference.handle]
    })
    const argumentsValue = {
      paths: [source],
      call_reason: 'Upload the workspace presentation in this original Tool call.'
    }
    try {
      await expect(
        host.callTool('browser_file_upload', argumentsValue, {
          authorizationContext: sensitiveContext('browser_file_upload', argumentsValue, {
            callId
          })
        })
      ).resolves.toMatchObject({ isError: false })
      const upstream = callTool.mock.calls[0]?.[0] as {
        arguments?: { paths?: string[]; _meta?: { cwd?: string } }
      }
      const stagedPath = upstream.arguments?.paths?.[0]
      expect(stagedPath).toBeTypeOf('string')
      expect(stagedPath).not.toBe(source)
      expect(stagedPath).not.toBe(reference.handle)
      expect(basename(stagedPath!)).toBe('浙江大学2026年招生资料汇编.pptx')
      expect(JSON.stringify(upstream)).not.toContain(source)
      expect(JSON.stringify(upstream)).not.toContain(reference.handle)
      expect(fileBroker.snapshot()).toMatchObject({
        handles: 0,
        retained: { leases: 1, files: 1 }
      })
      await host.releaseRun(RISK_CONTEXT.runId)
      await expect(access(stagedPath!)).rejects.toMatchObject({ code: 'ENOENT' })
    } finally {
      await fileBroker.shutdown()
      await rm(parent, { recursive: true, force: true })
    }
  })

  it('drops an approved large file through a same-frame file input without sending base64 upstream', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-large-drop-test-'))
    const source = join(parent, 'large-drop.bin')
    await writeFile(source, Buffer.alloc(1_100_000, 'L'))
    const fileBroker = new BrowserFileBroker({
      rootDirectory: join(parent, 'browser-automation-files'),
      selectionProvider: { selectFiles: vi.fn(async () => [source]) }
    })
    await fileBroker.initialize()
    const inputElement = {
      setInputFiles: vi.fn<(paths: readonly string[]) => Promise<void>>(async () => undefined),
      evaluate: vi.fn<(callback: unknown) => Promise<void>>(async () => undefined),
      dispose: vi.fn(async () => undefined)
    }
    const targetElement = {
      evaluateHandle: vi.fn<(callback: unknown) => Promise<{ asElement(): typeof inputElement }>>(
        async () => ({ asElement: () => inputElement })
      ),
      evaluate: vi.fn<
        (callback: unknown, argument: unknown) => Promise<{ accepted: boolean; fileCount: number }>
      >(async () => ({ accepted: true, fileCount: 1 })),
      dispose: vi.fn(async () => undefined)
    }
    const locator = { elementHandle: vi.fn(async () => targetElement) }
    const page = { locator: vi.fn(() => locator) }
    const context = {
      ...fakeBrowserContext(),
      pages: vi.fn(() => [page])
    } as unknown as BrowserContext
    const callTool = vi.fn(async () => ({
      content: [
        {
          type: 'text' as const,
          text: '- region "Drop completed" [ref=e42]\n  - text: fixture-large-drop.bin'
        }
      ],
      isError: false
    }))
    const host = fakeHost({
      callTool: callTool as ManagedMcpClient['callTool'],
      fileBroker,
      getBrowserContext: vi.fn(async () => context)
    })
    try {
      const [reference] = await fileBroker.freezeResolvedForRead({
        owner: {
          runId: RISK_CONTEXT.runId,
          activationId: RISK_CONTEXT.activationId,
          capabilityId: 'browser_automation',
          toolCallId: 'call-large-drop'
        },
        paths: [source]
      })
      const handle = reference.handle

      const dropArguments = {
        target: "frameLocator('iframe').locator('[data-drop-zone]')",
        paths: [handle!],
        data: { 'text/plain': 'approved-fixture' },
        call_reason: 'Drop the approved large fixture into the exact iframe target.'
      }
      await expect(
        host.callTool('browser_drop', dropArguments, {
          authorizationContext: sensitiveContext('browser_drop', dropArguments, {
            callId: 'call-large-drop'
          })
        })
      ).resolves.toMatchObject({
        content: expect.arrayContaining([
          expect.objectContaining({ type: 'text', text: expect.stringContaining('[ref=e42]') })
        ]),
        structuredContent: { status: 'completed', fileCount: 1, snapshotIncluded: true },
        isError: false
      })

      expect(callTool).toHaveBeenCalledOnce()
      expect(callTool).toHaveBeenCalledWith(
        { name: 'browser_snapshot', arguments: {} },
        undefined,
        expect.any(Object)
      )
      expect(page.locator).toHaveBeenCalledWith(
        'iframe >> internal:control=enter-frame >> [data-drop-zone]'
      )
      const stagedPaths = inputElement.setInputFiles.mock.calls[0]?.[0] as string[]
      expect(stagedPaths).toHaveLength(1)
      expect(stagedPaths[0]).not.toBe(source)
      expect(basename(stagedPaths[0])).toBe('large-drop.bin')
      expect(targetElement.evaluate.mock.calls[0]?.[1]).toMatchObject({
        data: { 'text/plain': 'approved-fixture' },
        fileInput: inputElement
      })
      expect(inputElement.evaluate).toHaveBeenCalledOnce()
      expect(inputElement.dispose).toHaveBeenCalledOnce()
      expect(targetElement.dispose).toHaveBeenCalledOnce()
      expect(fileBroker.snapshot().retained).toMatchObject({ leases: 1, files: 1 })

      await host.releaseRun(RISK_CONTEXT.runId)
      await expect(access(stagedPaths[0])).rejects.toMatchObject({ code: 'ENOENT' })
    } finally {
      await fileBroker.shutdown()
      await rm(parent, { recursive: true, force: true })
    }
  })

  it('normalizes an authoritative download-navigation error into a successful download start', async () => {
    const progress = [
      {
        downloadId: 'browser-download:123e4567-e89b-42d3-a456-426614174000',
        displayName: 'installer.dmg',
        mimeType: 'application/x-apple-diskimage',
        state: 'progressing' as const,
        receivedBytes: 1024,
        totalBytes: 419 * 1024 * 1024,
        bytesPerSecond: 512,
        startedAt: 1,
        updatedAt: 2
      }
    ]
    const risk = riskLease({ downloadProgress: progress })
    const host = fakeHost({
      beginNetworkOperation: vi.fn(async () => risk.lease),
      callTool: vi.fn(async () => ({
        content: [{ type: 'text', text: 'Error: Download is starting' }],
        isError: true
      })),
      getAgentDownloadSnapshot: () => ({
        schemaVersion: 2,
        revision: 1,
        downloads: progress
      })
    })

    await expect(
      host.callTool(
        'browser_click',
        { target: 'download', call_reason: 'Download the installer.' },
        { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
      )
    ).resolves.toMatchObject({
      structuredContent: {
        status: 'download_started',
        downloadProgress: [
          {
            displayName: 'installer.dmg',
            state: 'progressing',
            totalBytes: 419 * 1024 * 1024
          }
        ]
      },
      isError: false
    })
  })

  it('normalizes an ordinary successful click when Electron authoritatively starts a download', async () => {
    const progress = [
      {
        downloadId: 'browser-download:123e4567-e89b-42d3-a456-426614174000',
        displayName: 'archive.zip',
        mimeType: 'application/zip',
        state: 'progressing' as const,
        receivedBytes: 32,
        totalBytes: 1024,
        bytesPerSecond: 16,
        startedAt: 1,
        updatedAt: 2
      }
    ]
    const risk = riskLease({ downloadProgress: progress })
    const host = fakeHost({
      beginNetworkOperation: vi.fn(async () => risk.lease),
      callTool: vi.fn(async () => ({
        content: [{ type: 'text', text: 'Clicked the download link.' }],
        isError: false
      })),
      getAgentDownloadSnapshot: () => ({
        schemaVersion: 2,
        revision: 1,
        downloads: progress
      })
    })

    await expect(
      host.callTool(
        'browser_click',
        { target: 'download', call_reason: 'Download the archive.' },
        { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
      )
    ).resolves.toMatchObject({
      structuredContent: {
        status: 'download_started',
        downloadProgress: [{ displayName: 'archive.zip', state: 'progressing' }]
      },
      isError: false
    })
  })

  it('preserves an exact Broker failure when the official click rejects after dispatch', async () => {
    const risk = riskLease({
      settle: async () => {
        throw new BrowserDownloadBrokerError('browser.download.too_large', 'possibly_dispatched')
      }
    })
    const host = fakeHost({
      beginNetworkOperation: vi.fn(async () => risk.lease),
      callTool: vi.fn(async () => {
        throw new Error('untrusted upstream click error')
      })
    })

    await expect(
      host.callTool(
        'browser_click',
        { target: 'download', call_reason: 'Download the fixture.' },
        { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
      )
    ).rejects.toMatchObject({
      code: 'mcp.builtin_playwright.output_too_large',
      dispatchCertainty: 'possibly_dispatched'
    })
  })

  it('reports current-task download progress through the no-navigation config tool', async () => {
    const downloadId = 'browser-download:123e4567-e89b-42d3-a456-426614174000'
    const host = fakeHost({
      getAgentDownloadSnapshot: () => ({
        schemaVersion: 2,
        revision: 3,
        downloads: [
          {
            downloadId,
            displayName: 'archive.zip',
            mimeType: 'application/zip',
            state: 'paused',
            receivedBytes: 10,
            totalBytes: 100,
            bytesPerSecond: 0,
            startedAt: 1,
            updatedAt: 2
          }
        ]
      })
    })

    const result = await host.callTool(
      'browser_get_config',
      { call_reason: 'Check the active download.' },
      {
        authorizationContext: {
          ...RISK_CONTEXT,
          triggerToolName: 'browser_get_config'
        }
      }
    )
    expect(result).toMatchObject({
      structuredContent: {
        downloadProgress: [{ downloadId, state: 'paused', receivedBytes: 10 }]
      },
      isError: false
    })
    expect(JSON.stringify(result)).not.toContain('/Users/')
  })

  it('retires terminal and rejected post-drop snapshots without replaying the file drop', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-drop-retirement-test-'))
    const source = join(parent, 'approved-drop.txt')
    await writeFile(source, 'approved-drop-fixture')
    const fileBroker = new BrowserFileBroker({
      rootDirectory: join(parent, 'browser-automation-files'),
      selectionProvider: { selectFiles: vi.fn(async () => []) }
    })
    await fileBroker.initialize()
    const inputElement = {
      setInputFiles: vi.fn<(paths: readonly string[]) => Promise<void>>(async () => undefined),
      evaluate: vi.fn<(callback: unknown) => Promise<void>>(async () => undefined),
      dispose: vi.fn(async () => undefined)
    }
    const targetElement = {
      evaluateHandle: vi.fn<(callback: unknown) => Promise<{ asElement(): typeof inputElement }>>(
        async () => ({ asElement: () => inputElement })
      ),
      evaluate: vi.fn<
        (callback: unknown, argument: unknown) => Promise<{ accepted: boolean; fileCount: number }>
      >(async () => ({ accepted: true, fileCount: 1 })),
      dispose: vi.fn(async () => undefined)
    }
    const locator = { elementHandle: vi.fn(async () => targetElement) }
    const page = { locator: vi.fn(() => locator) }
    const context = {
      ...fakeBrowserContext(),
      pages: vi.fn(() => [page])
    } as unknown as BrowserContext
    const callTool = vi
      .fn<ManagedMcpClient['callTool']>()
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'Target page, context or browser has been closed' }],
        structuredContent: { errorCode: 'target_closed' },
        isError: true
      })
      .mockRejectedValueOnce(new Error('fixture post-drop transport closed'))
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'fresh snapshot' }],
        isError: false
      })
    const createOfficialConnection = vi.fn(async () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }))
    const detachAutomation = vi.fn(async () => undefined)
    const host = fakeHost({
      callTool,
      createOfficialConnection,
      detachAutomation,
      fileBroker,
      getBrowserContext: vi.fn(async () => context)
    })
    const freezeDropHandle = async (callId: string): Promise<string> => {
      const [reference] = await fileBroker.freezeResolvedForRead({
        owner: {
          runId: RISK_CONTEXT.runId,
          activationId: RISK_CONTEXT.activationId,
          capabilityId: 'browser_automation',
          toolCallId: callId
        },
        paths: [source]
      })
      return reference.handle!
    }

    try {
      const terminalCallId = 'call-drop-terminal-snapshot'
      const terminalArguments = {
        target: 'e17',
        paths: [await freezeDropHandle(terminalCallId)],
        call_reason: 'Exercise a terminal post-drop snapshot response.'
      }
      await expect(
        host.callTool('browser_drop', terminalArguments, {
          authorizationContext: sensitiveContext('browser_drop', terminalArguments, {
            callId: terminalCallId
          })
        })
      ).resolves.toMatchObject({
        structuredContent: { status: 'completed', fileCount: 1, snapshotIncluded: false },
        isError: false
      })
      expect(callTool).toHaveBeenCalledOnce()
      expect(createOfficialConnection).toHaveBeenCalledOnce()
      expect(detachAutomation).toHaveBeenCalledOnce()

      const rejectedCallId = 'call-drop-rejected-snapshot'
      const rejectedArguments = {
        target: 'e17',
        paths: [await freezeDropHandle(rejectedCallId)],
        call_reason: 'Exercise a rejected post-drop snapshot request.'
      }
      await expect(
        host.callTool('browser_drop', rejectedArguments, {
          authorizationContext: sensitiveContext('browser_drop', rejectedArguments, {
            callId: rejectedCallId
          })
        })
      ).rejects.toMatchObject({ dispatchCertainty: 'possibly_dispatched' })
      expect(callTool).toHaveBeenCalledTimes(2)
      expect(createOfficialConnection).toHaveBeenCalledTimes(2)
      expect(detachAutomation).toHaveBeenCalledTimes(2)

      await expect(
        host.callTool('browser_snapshot', {
          call_reason: 'Read a fresh fixture after the rejected post-drop snapshot.'
        })
      ).resolves.toMatchObject({ isError: false })
      expect(callTool).toHaveBeenCalledTimes(3)
      expect(createOfficialConnection).toHaveBeenCalledTimes(3)
      expect(inputElement.setInputFiles).toHaveBeenCalledTimes(2)
    } finally {
      await fileBroker.shutdown()
      await rm(parent, { recursive: true, force: true })
    }
  })
})
