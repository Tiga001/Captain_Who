import { EventEmitter } from 'node:events'
import type { Debugger, WebContents } from 'electron'
import { describe, expect, it, vi } from 'vitest'

import { ElectronGuestCdpTransport } from '../browser/ElectronGuestCdpTransport'

class DebuggerFixture extends EventEmitter {
  attached = false
  historyEntries: Array<{ id: number; title: string; transitionType: string; url: string }> = []

  attach = vi.fn(() => {
    this.attached = true
  })
  detach = vi.fn(() => {
    this.attached = false
  })
  isAttached = vi.fn(() => this.attached)
  sendCommand = vi.fn(async (method: string) => {
    if (method === 'Browser.getVersion') {
      return {
        jsVersion: '1',
        product: 'Chrome/142.0.0.0',
        protocolVersion: '1.3',
        revision: 'fixture',
        userAgent: 'fixture'
      }
    }
    if (method === 'Target.getTargetInfo') {
      return {
        targetInfo: {
          attached: true,
          browserContextId: 'raw-context',
          canAccessOpener: false,
          targetId: 'raw-target',
          title: 'Physical internal page',
          type: 'page',
          url: this.currentUrl
        }
      }
    }
    if (method === 'Page.getNavigationHistory') {
      return { currentIndex: this.historyEntries.length - 1, entries: this.historyEntries }
    }
    if (method === 'Page.getFrameTree' || method === 'Page.getResourceTree') {
      return {
        frameTree: {
          frame: {
            domainAndRegistry: 'page',
            id: 'main',
            loaderId: 'loader-main',
            securityOrigin: 'mycopilot-browser-internal://page',
            unreachableUrl: this.currentUrl,
            url: this.currentUrl
          },
          childFrames: [
            { frame: { id: 'child', loaderId: 'loader-child', url: 'data:text/html,forged' } }
          ],
          resources: [
            { mimeType: 'text/html', type: 'Document', url: this.currentUrl },
            { mimeType: 'text/html', type: 'Document', url: 'data:text/html,forged' }
          ]
        }
      }
    }
    return {}
  })

  constructor(public currentUrl: string) {
    super()
  }
}

class GuestFixture extends EventEmitter {
  destroyed = false
  readonly hostWebContents = { focus: vi.fn() }
  readonly focus = vi.fn()

  constructor(
    public url: string,
    readonly debuggerFixture: DebuggerFixture
  ) {
    super()
  }

  get debugger(): Debugger {
    return this.debuggerFixture as unknown as Debugger
  }

  getTitle(): string {
    return 'Physical internal page'
  }

  getURL(): string {
    return this.url
  }

  isDestroyed(): boolean {
    return this.destroyed
  }

  asWebContents(): WebContents {
    return this as unknown as WebContents
  }
}

describe('ElectronGuestCdpTransport logical presentation', () => {
  it('keeps target identity stable and never exposes private physical documents', async () => {
    const pageA = 'https://a.example.test/'
    const pageB = 'https://b.example.test/private?token=secret'
    const internalUrl = `mycopilot-browser-internal://page/${'A'.repeat(32)}`
    const forgedDataUrl = 'data:text/html;charset=utf-8,%3Ch1%3Eforged%3C%2Fh1%3E'
    const debuggerFixture = new DebuggerFixture(internalUrl)
    debuggerFixture.historyEntries = [
      { id: 1, title: 'A', transitionType: 'link', url: pageA },
      { id: 2, title: 'B failed', transitionType: 'link', url: internalUrl },
      { id: 3, title: 'forged', transitionType: 'link', url: forgedDataUrl }
    ]
    const guest = new GuestFixture(internalUrl, debuggerFixture)
    const transport = new ElectronGuestCdpTransport(
      guest.asWebContents(),
      vi.fn(),
      (physicalUrl) =>
        physicalUrl === internalUrl ? { title: 'Unable to reach this site', url: pageB } : null
    )
    const messages: Record<string, unknown>[] = []
    const pending = new Map<number, (message: Record<string, unknown>) => void>()
    transport.onmessage = (message) => {
      const record = message as Record<string, unknown>
      messages.push(record)
      if (typeof record.id === 'number') {
        pending.get(record.id)?.(record)
        pending.delete(record.id)
      }
    }
    const send = async (
      id: number,
      method: string,
      sessionId?: string
    ): Promise<Record<string, unknown>> =>
      await new Promise((resolve) => {
        pending.set(id, resolve)
        transport.send({ id, method, ...(sessionId ? { sessionId } : {}) })
      })

    await transport.attach()
    const identityBefore = transport.managedIdentity()
    expect(identityBefore.targetInfo).toMatchObject({
      targetId: 'raw-target',
      title: 'Unable to reach this site',
      url: pageB
    })

    const targetResponse = await send(1, 'Target.getTargetInfo')
    const historyResponse = await send(2, 'Page.getNavigationHistory', identityBefore.sessionId)
    const frameTreeResponse = await send(3, 'Page.getFrameTree', identityBefore.sessionId)
    const resourceTreeResponse = await send(4, 'Page.getResourceTree', identityBefore.sessionId)
    debuggerFixture.emit(
      'message',
      {},
      'Page.frameNavigated',
      {
        frame: {
          domainAndRegistry: 'page',
          id: 'main',
          loaderId: 'loader-main',
          securityOrigin: 'mycopilot-browser-internal://page',
          unreachableUrl: internalUrl,
          url: internalUrl
        }
      },
      ''
    )
    debuggerFixture.emit(
      'message',
      {},
      'Network.requestWillBeSent',
      {
        documentURL: internalUrl,
        request: { method: 'GET', url: internalUrl },
        requestId: 'request-1',
        type: 'Document'
      },
      ''
    )
    debuggerFixture.emit(
      'message',
      {},
      'Runtime.executionContextCreated',
      {
        context: {
          id: 1,
          name: '',
          origin: 'mycopilot-browser-internal://page',
          uniqueId: 'context-1'
        }
      },
      ''
    )
    debuggerFixture.emit(
      'message',
      {},
      'Debugger.scriptParsed',
      { scriptId: '1', url: internalUrl },
      ''
    )
    debuggerFixture.emit(
      'message',
      {},
      'Security.certificateError',
      { eventId: 1, errorType: 'certificate', requestURL: internalUrl },
      ''
    )

    const serialized = JSON.stringify({
      frameTreeResponse,
      historyResponse,
      messages,
      resourceTreeResponse,
      targetResponse
    })
    expect(serialized).not.toContain('mycopilot-browser-internal:')
    expect(serialized).not.toContain('data:text/html')
    expect(serialized).not.toContain('%3Ch1%3E')
    expect(serialized).toContain(pageA)
    expect(serialized).toContain(pageB)
    expect(serialized).toContain('about:blank')
    expect(transport.managedIdentity().targetId).toBe(identityBefore.targetId)

    transport.close()
  })
})
