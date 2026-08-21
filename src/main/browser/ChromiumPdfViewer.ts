import type { OnBeforeRequestListenerDetails, WebContents } from 'electron'

const CHROMIUM_PDF_VIEWER_EXTENSION_ID = 'mhjfbmdgcfjbbpaeojofohoefgiehjai'
const CHROMIUM_PDF_VIEWER_ENTRY_PATH = '/index.html'
const MAX_TRACKED_PDF_VIEWERS = 32

type ChromiumPdfViewerRequest = Pick<
  OnBeforeRequestListenerDetails,
  'method' | 'resourceType' | 'url' | 'webContents' | 'webContentsId'
>

/**
 * Admits the private resources used by Chromium's compiled-in PDF Viewer component.
 *
 * The viewer runs in an internal WebContents that is intentionally absent from the managed
 * Browser surface registry. Trust starts only when that WebContents requests the exact component
 * entry point; subsequent access stays bound to the same Electron identity.
 */
export class ChromiumPdfViewerRequestGate {
  private readonly viewers = new Map<
    number,
    { contents?: WebContents; handleDestroyed?: () => void }
  >()

  allows(details: ChromiumPdfViewerRequest): boolean {
    const identity = requestIdentity(details)
    if (!identity) return false

    if (isChromiumPdfViewerEntryRequest(details)) {
      this.remember(identity.id, identity.contents)
      return true
    }

    const existing = this.viewers.get(identity.id)
    if (!existing || !isChromiumPdfViewerResourceRequest(details)) return false
    if (!existing.contents && identity.contents) this.remember(identity.id, identity.contents)
    return true
  }

  shutdown(): void {
    for (const id of [...this.viewers.keys()]) this.forget(id)
  }

  private remember(id: number, contents?: WebContents): void {
    const current = this.viewers.get(id)
    if (current && (current.contents === contents || (current.contents && !contents))) return
    if (current) this.forget(id)

    while (this.viewers.size >= MAX_TRACKED_PDF_VIEWERS) {
      const oldestId = this.viewers.keys().next().value as number | undefined
      if (oldestId === undefined) break
      this.forget(oldestId)
    }

    if (!contents || contents.isDestroyed()) {
      this.viewers.set(id, {})
      return
    }

    const handleDestroyed = (): void => {
      if (this.viewers.get(id)?.contents === contents) this.viewers.delete(id)
    }
    contents.once('destroyed', handleDestroyed)
    this.viewers.set(id, { contents, handleDestroyed })
  }

  private forget(id: number): void {
    const record = this.viewers.get(id)
    if (!record) return
    if (record.contents && record.handleDestroyed && !record.contents.isDestroyed()) {
      record.contents.removeListener('destroyed', record.handleDestroyed)
    }
    this.viewers.delete(id)
  }
}

export function isChromiumPdfViewerEntryRequest(
  details: Pick<ChromiumPdfViewerRequest, 'method' | 'resourceType' | 'url'>
): boolean {
  if (details.method !== 'GET' || details.resourceType !== 'mainFrame') return false

  const url = parseUrl(details.url)
  return (
    url !== null && isPdfViewerExtensionUrl(url) && url.pathname === CHROMIUM_PDF_VIEWER_ENTRY_PATH
  )
}

function isChromiumPdfViewerResourceRequest(details: ChromiumPdfViewerRequest): boolean {
  if (details.method !== 'GET') return false

  const url = parseUrl(details.url)
  if (!url) return false
  if (isPdfViewerExtensionUrl(url)) {
    return details.resourceType !== 'mainFrame' || url.pathname === CHROMIUM_PDF_VIEWER_ENTRY_PATH
  }
  return (
    details.resourceType !== 'mainFrame' &&
    url.protocol === 'chrome:' &&
    url.hostname === 'resources' &&
    hasUnprivilegedUrlShape(url)
  )
}

function requestIdentity(
  details: Pick<ChromiumPdfViewerRequest, 'webContents' | 'webContentsId'>
): { contents?: WebContents; id: number } | null {
  const objectId = details.webContents?.id
  const explicitId = details.webContentsId
  if (objectId !== undefined && explicitId !== undefined && objectId !== explicitId) return null

  const id = explicitId ?? objectId
  if (id === undefined || !Number.isSafeInteger(id) || id <= 0) return null
  return { ...(details.webContents ? { contents: details.webContents } : {}), id }
}

function parseUrl(value: string): URL | null {
  try {
    return new URL(value)
  } catch {
    return null
  }
}

function isPdfViewerExtensionUrl(url: URL): boolean {
  return (
    url.protocol === 'chrome-extension:' &&
    url.hostname === CHROMIUM_PDF_VIEWER_EXTENSION_ID &&
    hasUnprivilegedUrlShape(url)
  )
}

function hasUnprivilegedUrlShape(url: URL): boolean {
  return url.port === '' && url.username === '' && url.password === ''
}
