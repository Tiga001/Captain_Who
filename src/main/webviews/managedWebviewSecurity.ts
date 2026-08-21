import { session } from 'electron'
import type {
  Event,
  WebContents,
  WebContentsDidStartNavigationEventParams,
  WebPreferences
} from 'electron'
import { BROWSER_WEBVIEW_PARTITION, parseBrowserSurfaceBootstrapUrl } from '@mycopilot/protocol'
import type {
  BrowserNetworkGuard,
  BrowserTargetCreationAuthority
} from '../browser/BrowserNetworkGuard'
import { isChromiumPdfViewerEntryRequest } from '../browser/ChromiumPdfViewer'

interface ManagedWebviewPolicy {
  allowedProtocols: ReadonlySet<string>
  newWindowBehavior: 'deny' | 'navigate-current'
  partition: string
}

export interface ManagedWebviewTargetRegistry {
  registerManagedGuest(input: {
    documentReady?: boolean
    guest: WebContents
    host: WebContents
    partition: string
    surfaceId?: string
  }): void
  handlePopup?(input: {
    authority?: BrowserTargetCreationAuthority
    guest: WebContents
    url: string
  }): Promise<void> | void
}

export interface ManagedWebviewHostOptions {
  networkGuard?: BrowserNetworkGuard
  targetRegistry?: ManagedWebviewTargetRegistry
}

export interface ManagedWebviewSessionOptions {
  networkGuard?: BrowserNetworkGuard
}

const SAFE_INITIAL_URL = 'about:blank'
const MANAGED_WEBVIEW_POLICIES: ManagedWebviewPolicy[] = [
  {
    allowedProtocols: new Set(['http:', 'https:']),
    newWindowBehavior: 'navigate-current',
    partition: BROWSER_WEBVIEW_PARTITION
  }
]

export function initializeManagedWebviewSessions(options: ManagedWebviewSessionOptions = {}): void {
  for (const policy of MANAGED_WEBVIEW_POLICIES) {
    const managedSession = session.fromPartition(policy.partition)
    managedSession.setPermissionCheckHandler(() => false)
    managedSession.setPermissionRequestHandler((_webContents, _permission, callback) => {
      callback(false)
    })
    if (options.networkGuard && policy.partition === BROWSER_WEBVIEW_PARTITION) {
      options.networkGuard.install()
    } else {
      managedSession.webRequest.onBeforeRequest((details, callback) => {
        if (
          details.resourceType === 'mainFrame' &&
          !isAllowedWebviewUrl(details.url, policy) &&
          !isChromiumPdfViewerEntryRequest(details)
        ) {
          callback({ cancel: true })
          return
        }

        callback({})
      })
    }
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
    if (!policy || !isAllowedInitialWebviewUrl(sourceUrl, policy)) {
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

    configureManagedGuest(guest, policy, options.networkGuard, options.targetRegistry)
    if (options.targetRegistry) {
      registerManagedGuestWhenIdentified(
        options.targetRegistry,
        {
          guest,
          host,
          partition: policy.partition
        },
        options.networkGuard
      )
    }
  })
}

function registerManagedGuestWhenIdentified(
  registry: ManagedWebviewTargetRegistry,
  input: { guest: WebContents; host: WebContents; partition: string },
  networkGuard?: BrowserNetworkGuard
): void {
  let settled = false
  let identifiedSurfaceId: string | null = null
  let provisionallyRegistered = false
  const timeout = setTimeout(() => failClosed(), 2_000)
  const cleanup = (): void => {
    clearTimeout(timeout)
    input.guest.removeListener('did-start-navigation', handleStartNavigation)
    input.guest.removeListener('dom-ready', handleReady)
    input.guest.removeListener('did-finish-load', handleReady)
    input.guest.removeListener('destroyed', cleanup)
  }
  const failClosed = (): void => {
    if (settled) return
    settled = true
    cleanup()
    if (!input.guest.isDestroyed()) input.guest.close()
  }
  const registerIdentifiedGuest = (surfaceId: string, documentReady: boolean): void => {
    if (settled || input.guest.isDestroyed()) return
    settled = true
    cleanup()
    try {
      // The bootstrap identity has already passed will-attach policy and strict protocol parsing.
      // Register it in the Main Surface map immediately; waiting for DOM readiness creates a
      // legitimate did-attach -> dom-ready interval where Renderer can only receive a false
      // "missing surface" result.
      registry.registerManagedGuest({ ...input, documentReady, surfaceId })
    } catch {
      if (!input.guest.isDestroyed()) input.guest.close()
    }
  }
  const rememberIdentity = (candidateUrl: string, documentReady = false): void => {
    if (settled || input.guest.isDestroyed()) return
    const surfaceId = parseBrowserSurfaceBootstrapUrl(candidateUrl)
    if (!surfaceId) return
    if (identifiedSurfaceId && identifiedSurfaceId !== surfaceId) {
      failClosed()
      return
    }
    identifiedSurfaceId = surfaceId
    if (networkGuard && !provisionallyRegistered) {
      try {
        // Network admission must begin as soon as the inert bootstrap identity is known. Waiting
        // for dom-ready leaves a did-attach -> registration window in which a real manual
        // navigation has an explicit but unknown WebContents identity and is correctly denied.
        networkGuard.registerGuest({ generation: 0, guest: input.guest, surfaceId })
        provisionallyRegistered = true
      } catch {
        failClosed()
        return
      }
    }
    registerIdentifiedGuest(surfaceId, documentReady)
  }
  const handleStartNavigation = (
    details: Event<WebContentsDidStartNavigationEventParams>
  ): void => {
    if (details.isMainFrame) rememberIdentity(details.url)
  }
  const handleReady = (): void => rememberIdentity(input.guest.getURL(), true)

  input.guest.on('did-start-navigation', handleStartNavigation)
  input.guest.on('dom-ready', handleReady)
  input.guest.on('did-finish-load', handleReady)
  input.guest.once('destroyed', cleanup)
  rememberIdentity(input.guest.getURL())
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

function configureManagedGuest(
  guest: WebContents,
  policy: ManagedWebviewPolicy,
  networkGuard?: BrowserNetworkGuard,
  targetRegistry?: ManagedWebviewTargetRegistry
): void {
  guest.setWindowOpenHandler(({ url }) => {
    if (
      policy.newWindowBehavior === 'navigate-current' &&
      isAllowedWebviewUrl(url, policy) &&
      url !== SAFE_INITIAL_URL
    ) {
      if (networkGuard) {
        if (targetRegistry?.handlePopup) {
          networkGuard.handleWindowOpen(guest, url, (authority) =>
            Promise.resolve(targetRegistry.handlePopup?.({ authority, guest, url }))
          )
        } else {
          networkGuard.handleWindowOpen(guest, url)
        }
      } else if (targetRegistry?.handlePopup) {
        void Promise.resolve(targetRegistry.handlePopup({ guest, url })).catch(() => undefined)
      } else {
        setImmediate(() => navigateManagedGuest(guest, url))
      }
    } else if (networkGuard && !isAllowedWebviewUrl(url, policy)) {
      networkGuard.recordBlockedNavigation(guest)
    }

    return { action: 'deny' }
  })

  guest.on('will-navigate', (event, url) => {
    if (!isAllowedWebviewUrl(url, policy)) {
      event.preventDefault()
      networkGuard?.recordBlockedNavigation(guest)
    }
  })

  guest.on('will-redirect', (event, url) => {
    if (!isAllowedWebviewUrl(url, policy)) {
      event.preventDefault()
      networkGuard?.recordBlockedNavigation(guest)
    }
  })
}

function navigateManagedGuest(guest: WebContents, url: string): void {
  if (guest.isDestroyed()) return

  void guest.loadURL(url).catch((error: unknown) => {
    if (!guest.isDestroyed() && !isAbortedNavigationError(error)) {
      // Electron errors can embed the full target URL, including sensitive query values.
      console.error('Managed webview navigation failed (safe error)')
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
  if (
    policy.partition === BROWSER_WEBVIEW_PARTITION &&
    parseBrowserSurfaceBootstrapUrl(value) !== null
  ) {
    return true
  }

  try {
    return policy.allowedProtocols.has(new URL(value).protocol)
  } catch {
    return false
  }
}

function isAllowedInitialWebviewUrl(value: string, policy: ManagedWebviewPolicy): boolean {
  return (
    isAllowedWebviewUrl(value, policy) ||
    (policy.partition === BROWSER_WEBVIEW_PARTITION &&
      parseBrowserSurfaceBootstrapUrl(value) !== null)
  )
}
