import { session } from 'electron'
import type { WebContents, WebPreferences } from 'electron'
import { BROWSER_WEBVIEW_PARTITION } from '@mycopilot/protocol'

interface ManagedWebviewPolicy {
  allowedProtocols: ReadonlySet<string>
  newWindowBehavior: 'deny' | 'navigate-current'
  partition: string
}

export interface ManagedWebviewTargetRegistry {
  registerManagedGuest(input: { guest: WebContents; host: WebContents; partition: string }): void
}

export interface ManagedWebviewHostOptions {
  targetRegistry?: ManagedWebviewTargetRegistry
}

const SAFE_INITIAL_URL = 'about:blank'
const MANAGED_WEBVIEW_POLICIES: ManagedWebviewPolicy[] = [
  {
    allowedProtocols: new Set(['http:', 'https:']),
    newWindowBehavior: 'navigate-current',
    partition: BROWSER_WEBVIEW_PARTITION
  }
]

export function initializeManagedWebviewSessions(): void {
  for (const policy of MANAGED_WEBVIEW_POLICIES) {
    const managedSession = session.fromPartition(policy.partition)
    managedSession.setPermissionCheckHandler(() => false)
    managedSession.setPermissionRequestHandler((_webContents, _permission, callback) => {
      callback(false)
    })
    managedSession.webRequest.onBeforeRequest((details, callback) => {
      if (details.resourceType === 'mainFrame' && !isAllowedWebviewUrl(details.url, policy)) {
        callback({ cancel: true })
        return
      }

      callback({})
    })
  }
}

export async function clearManagedWebviewData(partition: string): Promise<void> {
  const policy = getManagedWebviewPolicy(partition)
  if (!policy) throw new Error('Unknown managed webview partition')

  const managedSession = session.fromPartition(policy.partition)
  await Promise.all([managedSession.clearCache(), managedSession.clearStorageData()])
}

export function configureManagedWebviewHost(
  host: WebContents,
  options: ManagedWebviewHostOptions = {}
): void {
  host.on('will-attach-webview', (event, webPreferences, params) => {
    const policy = getManagedWebviewPolicy(params.partition)
    const sourceUrl = params.src || SAFE_INITIAL_URL
    if (!policy || !isAllowedWebviewUrl(sourceUrl, policy)) {
      event.preventDefault()
      console.warn('Blocked untrusted managed webview attachment')
      return
    }

    enforceManagedWebPreferences(webPreferences, policy)
    const requestedNewWindowHandling = 'allowpopups' in params
    if (policy.newWindowBehavior === 'navigate-current' && requestedNewWindowHandling) {
      // Enables the window-open event only; the guest handler still denies the real popup.
      params.allowpopups = 'true'
    } else {
      delete params.allowpopups
    }
    delete params.preload
  })

  host.on('did-attach-webview', (_event, guest) => {
    const policy = MANAGED_WEBVIEW_POLICIES.find(
      (candidate) => session.fromPartition(candidate.partition) === guest.session
    )
    if (!policy) {
      guest.close()
      return
    }

    configureManagedGuest(guest, policy)
    try {
      options.targetRegistry?.registerManagedGuest({ guest, host, partition: policy.partition })
    } catch {
      // Registration is part of the security boundary. A guest that cannot be tracked must not run.
      guest.close()
    }
  })
}

function enforceManagedWebPreferences(
  webPreferences: WebPreferences,
  policy: ManagedWebviewPolicy
): void {
  delete webPreferences.preload
  webPreferences.allowRunningInsecureContent = false
  webPreferences.contextIsolation = true
  webPreferences.experimentalFeatures = false
  webPreferences.navigateOnDragDrop = false
  webPreferences.nodeIntegration = false
  webPreferences.nodeIntegrationInSubFrames = false
  webPreferences.nodeIntegrationInWorker = false
  webPreferences.partition = policy.partition
  webPreferences.plugins = false
  webPreferences.safeDialogs = true
  webPreferences.sandbox = true
  webPreferences.webSecurity = true
  webPreferences.webviewTag = false
}

function configureManagedGuest(guest: WebContents, policy: ManagedWebviewPolicy): void {
  guest.setWindowOpenHandler(({ url }) => {
    if (
      policy.newWindowBehavior === 'navigate-current' &&
      isAllowedWebviewUrl(url, policy) &&
      url !== SAFE_INITIAL_URL
    ) {
      setImmediate(() => navigateManagedGuest(guest, url))
    }

    return { action: 'deny' }
  })

  guest.on('will-navigate', (event, url) => {
    if (!isAllowedWebviewUrl(url, policy)) event.preventDefault()
  })

  guest.on('will-redirect', (event, url) => {
    if (!isAllowedWebviewUrl(url, policy)) event.preventDefault()
  })
}

function navigateManagedGuest(guest: WebContents, url: string): void {
  if (guest.isDestroyed()) return

  void guest.loadURL(url).catch((error: unknown) => {
    if (!guest.isDestroyed() && !isAbortedNavigationError(error)) {
      console.error('Failed to navigate managed webview link', error)
    }
  })
}

function isAbortedNavigationError(error: unknown): boolean {
  return error instanceof Error && error.message.includes('ERR_ABORTED')
}

function getManagedWebviewPolicy(partition: string | undefined): ManagedWebviewPolicy | null {
  return MANAGED_WEBVIEW_POLICIES.find((policy) => policy.partition === partition) ?? null
}

function isAllowedWebviewUrl(value: string, policy: ManagedWebviewPolicy): boolean {
  if (value === SAFE_INITIAL_URL) return true

  try {
    return policy.allowedProtocols.has(new URL(value).protocol)
  } catch {
    return false
  }
}
