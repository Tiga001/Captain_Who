import { randomUUID } from 'node:crypto'
import { once } from 'node:events'
import { mkdirSync } from 'node:fs'
import { createServer } from 'node:http'
import {
  BROWSER_WEBVIEW_PARTITION,
  createBrowserSurfaceBootstrapUrl,
  type BrowserSurfaceCommand
} from '@mycopilot/protocol'
import { app, BrowserWindow, session, webContents, type WebContents } from 'electron'
import { BrowserInternalPageStore } from '../BrowserInternalPageStore'
import { BrowserNetworkGuard } from '../BrowserNetworkGuard'
import { BrowserNetworkPolicy, ElectronSessionDnsResolver } from '../BrowserNetworkPolicy'
import {
  BrowserRiskCoordinator,
  type BrowserRiskAuthorizationDecision,
  type BrowserRiskOperationInput
} from '../BrowserRiskCoordinator'
import { BrowserSurfaceManager } from '../BrowserSurfaceManager'
import { BrowserTargetBroker } from '../BrowserTargetBroker'
import {
  configureManagedWebviewHost,
  initializeManagedWebviewSessions
} from '../../webviews/managedWebviewSecurity'

const ROOT_SURFACE = 'native-popup-fixture-root'
const profile = process.argv[2]
if (!profile) throw new Error('fixture profile missing')
mkdirSync(profile, { recursive: true, mode: 0o700 })
app.setPath('userData', profile)
app.on('browser-window-created', (_event, window) => {
  window.setOpacity(0)
  window.setSkipTaskbar(true)
})

async function waitFor(predicate: () => boolean | Promise<boolean>, label: string): Promise<void> {
  const deadline = Date.now() + 10_000
  while (!(await predicate())) {
    if (Date.now() > deadline) throw new Error(`Timed out: ${label}`)
    await new Promise((resolve) => setTimeout(resolve, 15))
  }
}

async function main(): Promise<void> {
  await app.whenReady()
  const requests: Array<{ method: string; path: string; body: string; referer: string | null }> = []
  let openerOrigin = ''
  const childHtml = (): string => `<!doctype html><html><body><h1>LOGIN_POPUP</h1><script>
    window.received=[];
    addEventListener('message', event => {
      received.push({data:event.data,origin:event.origin,sameSource:event.source===opener});
      if(event.data==='ping' && opener) opener.postMessage({phase:'pong',path:location.pathname},${JSON.stringify(openerOrigin)});
      if(event.data==='close') window.close();
    });
    if(opener) opener.postMessage({phase:'loaded',path:location.pathname},${JSON.stringify(openerOrigin)});
  </script></body></html>`
  const sourceServer = createServer(async (request, response) => {
    let body = ''
    for await (const chunk of request) body += String(chunk)
    requests.push({
      method: request.method ?? '',
      path: request.url ?? '',
      body,
      referer: request.headers.referer ?? null
    })
    response.setHeader('content-type', 'text/html; charset=utf-8')
    if (request.url === '/opener') {
      response.end(`<!doctype html><html><body><h1>LOGIN_OPENER</h1><script>
        window.refs={}; window.messages=[];
        addEventListener('message', event => messages.push({data:event.data,origin:event.origin,
          sourceName:Object.keys(refs).find(name=>refs[name]===event.source)??null}));
      </script></body></html>`)
    } else response.end(childHtml())
  })
  sourceServer.listen(0, '127.0.0.1')
  await once(sourceServer, 'listening')
  const sourceAddress = sourceServer.address()
  if (!sourceAddress || typeof sourceAddress === 'string') throw new Error('source address missing')
  openerOrigin = `http://127.0.0.1:${sourceAddress.port}`
  const crossServer = createServer(async (request, response) => {
    let body = ''
    for await (const chunk of request) body += String(chunk)
    requests.push({
      method: request.method ?? '',
      path: request.url ?? '',
      body,
      referer: request.headers.referer ?? null
    })
    response.setHeader('content-type', 'text/html; charset=utf-8')
    if (request.url === '/coop') response.setHeader('Cross-Origin-Opener-Policy', 'same-origin')
    response.end(childHtml())
  })
  crossServer.listen(0, '127.0.0.1')
  await once(crossServer, 'listening')
  const crossAddress = crossServer.address()
  if (!crossAddress || typeof crossAddress === 'string') throw new Error('cross address missing')
  const crossOrigin = `http://localhost:${crossAddress.port}`

  const managedSession = session.fromPartition(BROWSER_WEBVIEW_PARTITION)
  const policy = new BrowserNetworkPolicy({
    dnsResolver: new ElectronSessionDnsResolver(managedSession)
  })
  let approvalGate:
    | {
        path: string
        requested: boolean
        release?: () => void
      }
    | undefined
  const coordinator = new BrowserRiskCoordinator({
    policy,
    authorizer: {
      authorize: async (request): Promise<BrowserRiskAuthorizationDecision> => {
        const gate = approvalGate
        if (
          gate &&
          !gate.requested &&
          request.trigger === 'new_window' &&
          new URL(request.destination.displayUrl).pathname === gate.path
        ) {
          gate.requested = true
          await new Promise<void>((resolve) => {
            gate.release = resolve
          })
        }
        return { decision: 'approved', grantId: randomUUID() }
      }
    }
  })
  const guard = new BrowserNetworkGuard({
    accessPolicy: 'risk_approval',
    coordinator,
    expectedSession: managedSession,
    policy
  })
  initializeManagedWebviewSessions({ networkGuard: guard })
  const internalPages = new BrowserInternalPageStore(managedSession)
  internalPages.install()
  const broker = new BrowserTargetBroker(BROWSER_WEBVIEW_PARTITION, managedSession)
  const host = new BrowserWindow({
    show: true,
    width: 600,
    height: 500,
    webPreferences: {
      webviewTag: true,
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true
    }
  })
  await host.loadURL('data:text/html,<html><body><div id="host"></div></body></html>')
  let rootGuest: WebContents | undefined
  let sequence = 0
  let selectionRevision = Date.now()
  let fatalRendererError: unknown
  let creationFailure: 'none' | 'factory' | 'configure' = 'none'
  const injectedFailures: Array<{
    mode: string
    windowCreated: boolean
    windowDestroyedAfterManagerFailure: boolean | null
    destroyedAfterManagerFailure: boolean
  }> = []
  const allocatedContents: WebContents[] = []
  app.on('web-contents-created', (_event, contents) => allocatedContents.push(contents))
  const uncaughtExceptions: string[] = []
  const recordUncaught = (error: Error): void => {
    uncaughtExceptions.push(error.message)
  }
  process.on('uncaughtException', recordUncaught)
  const manager = new BrowserSurfaceManager({
    broker,
    maxSurfaces: 2,
    networkGuard: guard,
    internalPageStore: internalPages,
    createSurfaceId: () => (sequence++ === 0 ? ROOT_SURFACE : `native-popup-fixture-${sequence}`),
    resolveHost: () => host.webContents,
    sendCommand: (_host, command) => {
      void applyRendererCommand(command).catch((error: unknown) => {
        fatalRendererError = error
      })
    }
  })
  configureManagedWebviewHost(host.webContents, {
    networkGuard: guard,
    targetRegistry: {
      registerManagedGuest: (input) => manager.registerManagedGuest(input),
      canCreateNativePopup: (input) => manager.canCreateNativePopup(input),
      isInternalNavigationAllowed: (guest, url) => manager.isInternalNavigationAllowed(guest, url),
      createNativePopup: (input) => {
        let constructedWindow: BrowserWindow | undefined
        try {
          return manager.createNativePopup({
            ...input,
            createWindow: (options) => {
              if (creationFailure === 'factory')
                throw new Error('fixture native window constructor failed')
              constructedWindow = input.createWindow(options)
              return constructedWindow
            },
            ...(creationFailure === 'configure'
              ? {
                  configureGuest: (guest: WebContents) => {
                    input.configureGuest(guest)
                    throw new Error('fixture native guest configuration failed')
                  }
                }
              : {})
          })
        } catch (error) {
          const nativeGuest = (input.options as typeof input.options & { webContents: WebContents })
            .webContents
          injectedFailures.push({
            mode: creationFailure,
            windowCreated: constructedWindow !== undefined,
            windowDestroyedAfterManagerFailure: constructedWindow?.isDestroyed() ?? null,
            destroyedAfterManagerFailure: nativeGuest.isDestroyed()
          })
          throw error
        }
      }
    }
  })
  host.webContents.on('did-attach-webview', (_event, guest) => {
    rootGuest = guest
  })

  async function applyRendererCommand(command: BrowserSurfaceCommand): Promise<void> {
    if (command.surfaceId !== ROOT_SURFACE)
      throw new Error('native popup unexpectedly requested a Renderer webview')
    if (command.kind === 'closeSurface') {
      await host.webContents.executeJavaScript("document.querySelector('webview')?.remove()")
      return
    }
    if (command.kind === 'ensureAttached' || command.kind === 'createSurface') {
      await host.webContents.executeJavaScript(`new Promise((resolve,reject)=>{
        const existing=document.querySelector('webview');if(existing){resolve();return}
        const view=document.createElement('webview');view.setAttribute('allowpopups','');
        view.setAttribute('partition',${JSON.stringify(BROWSER_WEBVIEW_PARTITION)});
        view.setAttribute('src',${JSON.stringify(createBrowserSurfaceBootstrapUrl(ROOT_SURFACE))});
        view.style.width='560px';view.style.height='400px';
        view.addEventListener('dom-ready',()=>resolve(),{once:true});
        document.querySelector('#host').append(view);
      })`)
    }
    await waitFor(
      () =>
        !!rootGuest && manager.listSurfaces().some((surface) => surface.surfaceId === ROOT_SURFACE),
      'root registration'
    )
    await waitFor(() => {
      const selected = manager.selectManualSurface(host.webContents, {
        schemaVersion: 1,
        surfaceId: ROOT_SURFACE,
        surfaceInstanceId: null,
        selectionRevision: ++selectionRevision
      })
      selectionRevision = Math.max(selectionRevision, selected.authoritativeRevision)
      if (!selected.surfaceInstanceId) return false
      const result = manager.attach(host.webContents, {
        schemaVersion: 1,
        requestId: command.requestId,
        surfaceId: ROOT_SURFACE,
        surfaceInstanceId: selected.surfaceInstanceId,
        ...(command.kind === 'resizeSurface'
          ? { viewport: { height: command.height, width: command.width } }
          : {})
      })
      return result.status === 'applied' || result.reason === 'already_ready'
    }, 'root Renderer ready acknowledgement')
  }

  const operationInput = (): BrowserRiskOperationInput => ({
    parentRequestId: randomUUID(),
    authorizationContext: {
      runId: 'native-popup-fixture-run',
      activationId: randomUUID(),
      capabilityId: 'browser_automation',
      manifestDigest: `sha256:${'a'.repeat(64)}`,
      policyRevision: 1,
      grantExpiresAtMs: Date.now() + 120_000,
      invocationId: randomUUID(),
      callId: randomUUID(),
      triggerToolName: 'browser_click',
      callReason: 'Verify a repository-owned login popup.'
    }
  })
  const results: Record<string, unknown>[] = []
  try {
    const context = await manager.getBrowserContext()
    const opener = context.pages()[0]
    await opener.goto(`${openerOrigin}/opener`)
    if (!rootGuest) throw new Error('root guest missing')
    const source = rootGuest

    for (const label of ['same', 'cross', 'blank', 'post', 'noopener', 'noreferrer', 'coop']) {
      // Exercise independent login flows without intentionally triggering the production
      // four-popups-per-second abuse limit.
      await new Promise((resolve) => setTimeout(resolve, 300))
      await manager.selectSurface({ surfaceId: ROOT_SURFACE })
      const path = `/${label}`
      const target = `${label === 'same' || label === 'post' ? openerOrigin : crossOrigin}${path}`
      const lease = await manager.beginNetworkOperation(operationInput())
      lease.markDispatched()
      const existing = new Set(manager.listSurfaces().map((surface) => surface.surfaceId))
      const gate = { path, requested: false } as NonNullable<typeof approvalGate>
      approvalGate = gate
      let opened: unknown
      try {
        if (label === 'post') {
          opened = await source.executeJavaScript(`(()=>{
            const form=document.createElement('form');form.method='POST';form.target='_blank';
            form.action=${JSON.stringify(target)};const input=document.createElement('input');
            input.name='state';input.value='preserved-original-body';form.append(input);document.body.append(form);form.submit();return true;
          })()`)
        } else {
          const features = label === 'noopener' || label === 'noreferrer' ? label : ''
          opened = await source.executeJavaScript(`(()=>{
            const popup=window.open(${JSON.stringify(label === 'blank' ? 'about:blank' : target)},'_blank',${JSON.stringify(features)});
            refs[${JSON.stringify(label)}]=popup;
            const initialOpener=${label === 'blank' ? 'popup.opener===window' : 'null'};
            ${label === 'blank' ? `popup.location.href=${JSON.stringify(target)};` : ''}
            return {isNull:popup===null,initialOpener};
          })()`)
        }
        // about:blank is admitted without a destination risk; its immediate location write must
        // still wait behind the native creation gate. Other popups pause their new_window check.
        if (label !== 'blank') {
          await waitFor(() => gate.requested, `${label} approval request`)
          await new Promise((resolve) => setTimeout(resolve, 100))
          if (requests.some((request) => request.path === path))
            throw new Error(`${label} request escaped popup admission`)
          gate.release?.()
        }
        await waitFor(
          () => context.pages().some((page) => page.url() === target),
          `${label} managed page admission`
        )
        const page = context.pages().find((candidate) => candidate.url() === target)
        if (!page) throw new Error(`${label} page missing`)
        await page.getByRole('heading', { name: 'LOGIN_POPUP' }).waitFor()
        const popupWindow = BrowserWindow.getAllWindows().find(
          (window) => window.webContents.getURL() === target
        )
        if (!popupWindow) throw new Error(`${label} native window missing`)
        await waitFor(() => popupWindow.isVisible(), `${label} native window visible`)
        const hostBounds = host.getBounds()
        const popupBounds = popupWindow.getBounds()
        const presentation = {
          width: popupBounds.width,
          height: popupBounds.height,
          centeredOnHost:
            Math.abs(popupBounds.x + popupBounds.width / 2 - hostBounds.x - hostBounds.width / 2) <=
              1 &&
            Math.abs(
              popupBounds.y + popupBounds.height / 2 - hostBounds.y - hostBounds.height / 2
            ) <= 1,
          titleMatchesUrl: popupWindow.getTitle() === `CaptainWho-${target}`,
          pageTitleCannotOverride: false,
          titleFollowsNavigation: false
        }
        await page.evaluate(() => {
          document.title = 'Website title must not replace the managed window title'
        })
        await waitFor(
          () =>
            popupWindow.webContents.getTitle() ===
            'Website title must not replace the managed window title',
          `${label} website title updated`
        )
        presentation.pageTitleCannotOverride = popupWindow.getTitle() === `CaptainWho-${target}`
        await page.evaluate(() => {
          history.replaceState(null, '', `${location.pathname}#login-step`)
        })
        await waitFor(
          () => popupWindow.getTitle() === `CaptainWho-${target}#login-step`,
          `${label} window title follows same-document navigation`
        )
        presentation.titleFollowsNavigation = true
        const surface = manager
          .listSurfaces()
          .find((candidate) => !existing.has(candidate.surfaceId))
        if (!surface) throw new Error(`${label} native surface missing`)
        const childHasOpener = await page.evaluate(() => window.opener !== null)
        let bidirectionalMessages = false
        if (label === 'same' || label === 'cross' || label === 'blank') {
          await source.executeJavaScript(
            `refs[${JSON.stringify(label)}].postMessage('ping',${JSON.stringify(new URL(target).origin)})`
          )
          await waitFor(
            async () =>
              await source.executeJavaScript(
                `messages.some(message=>message.data.phase==='pong'&&message.data.path===${JSON.stringify(path)}&&message.origin===${JSON.stringify(new URL(target).origin)}&&message.sourceName===${JSON.stringify(label)})`
              ),
            `${label} return message identity`
          )
          bidirectionalMessages = await page.evaluate((expectedOrigin) => {
            const state = window as unknown as {
              received: Array<{ data: string; origin: string; sameSource: boolean }>
            }
            return state.received.some(
              (message) =>
                message.data === 'ping' && message.origin === expectedOrigin && message.sameSource
            )
          }, openerOrigin)
        }
        await lease.settle()
        lease.finish()
        await manager.selectSurface({ surfaceId: surface.surfaceId })
        const selected = manager.getActiveSurfaceIdentity()?.surfaceId === surface.surfaceId
        let proxyClosed: boolean | null = null
        if (label === 'coop') proxyClosed = await source.executeJavaScript(`refs.coop.closed`)
        if (label === 'same' || label === 'cross' || label === 'blank') {
          await page.evaluate(() => window.close())
          await waitFor(
            () =>
              !manager
                .listSurfaces()
                .some((candidate) => candidate.surfaceId === surface.surfaceId),
            `${label} native window.close`
          )
          proxyClosed = await source.executeJavaScript(`refs[${JSON.stringify(label)}].closed`)
        } else await manager.closeSurface(surface.surfaceId)
        results.push({
          label,
          opened,
          childHasOpener,
          bidirectionalMessages,
          presentation,
          selected,
          proxyClosed,
          surfaceRemoved: !manager
            .listSurfaces()
            .some((candidate) => candidate.surfaceId === surface.surfaceId),
          openerSurvived: !source.isDestroyed() && !host.isDestroyed()
        })
      } finally {
        gate.release?.()
        approvalGate = undefined
        lease.finish()
      }
    }
    // Expected capacity denial happens before Chromium allocates a child. Unexpected factory or
    // post-construction failures must dispose that exact already-allocated native child without
    // allowing its original request or throwing out of Electron's createWindow callback.
    await new Promise((resolve) => setTimeout(resolve, 1_000))
    await manager.selectSurface({ surfaceId: ROOT_SURFACE })
    await source.executeJavaScript(
      `Boolean(window.open(${JSON.stringify(`${openerOrigin}/capacity-holder`)},'_blank'))`
    )
    await waitFor(() => manager.listSurfaces().length === 2, 'capacity holder surface')
    await waitFor(
      () => context.pages().some((page) => page.url() === `${openerOrigin}/capacity-holder`),
      'capacity holder page'
    )
    const beforeCapacity = allocatedContents.length
    const capacityReturnedNull = await source.executeJavaScript(
      `window.open(${JSON.stringify(`${openerOrigin}/capacity-denied`)},'_blank')===null`
    )
    await new Promise((resolve) => setTimeout(resolve, 100))
    const capacityCreatedNoGuest = allocatedContents.length === beforeCapacity
    const holder = manager.listSurfaces().find((surface) => surface.surfaceId !== ROOT_SURFACE)
    if (!holder) throw new Error('capacity holder disappeared')
    await manager.closeSurface(holder.surfaceId)
    const failureResults: Array<Record<string, unknown>> = []
    for (const mode of ['factory', 'configure'] as const) {
      await new Promise((resolve) => setTimeout(resolve, 350))
      creationFailure = mode
      const before = new Set(webContents.getAllWebContents().map((contents) => contents.id))
      const allocatedBefore = allocatedContents.length
      await source.executeJavaScript(
        `refs.failure=window.open(${JSON.stringify(`${openerOrigin}/failure-${mode}`)},'_blank');true`
      )
      await waitFor(() => allocatedContents.length > allocatedBefore, `${mode} native allocation`)
      await waitFor(
        () => allocatedContents.slice(allocatedBefore).every((contents) => contents.isDestroyed()),
        `${mode} rejected guest cleanup`
      )
      await waitFor(
        async () =>
          await source.executeJavaScript('refs.failure===null||refs.failure?.closed===true'),
        `${mode} rejected proxy close`
      )
      failureResults.push({
        mode,
        noOrphanContents: webContents
          .getAllWebContents()
          .every((contents) => before.has(contents.id)),
        noRequest: !requests.some((request) => request.path === `/failure-${mode}`),
        surfaceCount: manager.listSurfaces().length,
        openerSurvived: !source.isDestroyed() && !host.isDestroyed()
      })
      creationFailure = 'none'
    }
    if (fatalRendererError) throw fatalRendererError
    const post = requests.filter((request) => request.path === '/post')
    const noreferrer = requests.filter((request) => request.path === '/noreferrer')
    console.log(
      `MYCOPILOT_NATIVE_POPUP_RESULT=${JSON.stringify({
        results,
        post,
        noreferrer,
        rejectedCreation: {
          capacityReturnedNull,
          capacityCreatedNoGuest,
          capacitySentNoRequest: !requests.some((request) => request.path === '/capacity-denied'),
          injectedFailures,
          failureResults,
          uncaughtExceptions
        },
        finalSurfaceCount: manager.listSurfaces().length,
        isolatedProfile: app.getPath('userData') === profile,
        noRemoteDebuggingPort: !process.argv.some((argument) =>
          argument.startsWith('--remote-debugging-port')
        )
      })}`
    )
  } finally {
    approvalGate?.release?.()
    await manager.shutdown().catch(() => undefined)
    await guard.shutdown()
    await coordinator.shutdown()
    process.removeListener('uncaughtException', recordUncaught)
    for (const window of BrowserWindow.getAllWindows()) if (!window.isDestroyed()) window.destroy()
    sourceServer.close()
    crossServer.close()
  }
}

main().then(
  () => app.quit(),
  (error: unknown) => {
    console.error(error)
    app.exit(1)
  }
)
