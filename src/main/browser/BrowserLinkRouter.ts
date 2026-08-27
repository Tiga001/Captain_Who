import { shell } from 'electron'
import {
  BROWSER_DATA_SCHEMA_VERSION,
  parseBrowserOpenUrlInput,
  parseBrowserPreferencesUpdateInput,
  type BrowserPreferencesView
} from '@mycopilot/protocol'

import type { CoreServer } from '../core/coreServer'
import type { BrowserSurfaceManager } from './BrowserSurfaceManager'

interface BrowserLinkRouterOptions {
  coreServer: CoreServer
  initialPreferences: BrowserPreferencesView
  surfaceManager: BrowserSurfaceManager
  openSystemUrl?: (url: string) => Promise<void>
}

/** Routes app-owned links without exposing the managed guest or its target identity. */
export class BrowserLinkRouter {
  private readonly coreServer: CoreServer
  private readonly surfaceManager: BrowserSurfaceManager
  private readonly openSystemUrl: (url: string) => Promise<void>
  private preferencesRecord: BrowserPreferencesView

  constructor(options: BrowserLinkRouterOptions) {
    this.coreServer = options.coreServer
    this.surfaceManager = options.surfaceManager
    this.preferencesRecord = structuredClone(options.initialPreferences)
    this.openSystemUrl = options.openSystemUrl ?? ((url) => shell.openExternal(url))
  }

  preferences(): BrowserPreferencesView {
    return structuredClone(this.preferencesRecord)
  }

  async updatePreferences(value: unknown): Promise<BrowserPreferencesView> {
    const input = parseBrowserPreferencesUpdateInput(value)
    if (input.linkOpenTarget === this.preferencesRecord.linkOpenTarget) return this.preferences()

    const saved = await this.coreServer.saveBrowserPreferences({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      linkOpenTarget: input.linkOpenTarget,
      expectedRevision: this.preferencesRecord.revision,
      updatedAt: Date.now()
    })
    this.preferencesRecord = saved
    return this.preferences()
  }

  async openAppUrl(value: unknown): Promise<void> {
    const url = parseAppUrl(value)
    if (url.protocol === 'mailto:' || this.preferencesRecord.linkOpenTarget === 'system') {
      await this.openSystemUrl(url.toString())
      return
    }
    await this.surfaceManager.createSurface({ activate: true, url: url.toString() })
  }

  async openInBuiltinBrowser(value: unknown): Promise<void> {
    const input = parseBrowserOpenUrlInput(value)
    await this.surfaceManager.createSurface({ activate: true, url: input.url })
  }
}

function parseAppUrl(value: unknown): URL {
  if (typeof value !== 'string') throw new Error('App URL must be a string')
  const url = new URL(value)
  if (!['http:', 'https:', 'mailto:'].includes(url.protocol)) {
    throw new Error(`Unsupported app URL protocol: ${url.protocol}`)
  }
  if (url.username || url.password) throw new Error('Credential-bearing app URLs are unsupported')
  return url
}
