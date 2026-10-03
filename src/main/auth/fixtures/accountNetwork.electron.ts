import { app, session } from 'electron'
import { readFileSync } from 'node:fs'
import { createServer as createHttpServer } from 'node:http'
import { createServer as createHttpsServer } from 'node:https'
import { connect, type AddressInfo, type Socket } from 'node:net'
import { join } from 'node:path'
import { createAccountFetch } from '../AccountNetwork'

const fixtureRoot = process.env.MYCOPILOT_ACCOUNT_NETWORK_FIXTURE_ROOT
if (!fixtureRoot) throw new Error('Missing account network fixture directory')
app.setPath('userData', join(fixtureRoot, 'user-data'))

async function run(): Promise<void> {
  await app.whenReady()
  app.dock?.hide()
  const sockets = new Set<Socket>()
  const received: Array<{ path: string; cookie: string; authorization: string }> = []
  const origin = createHttpsServer(
    {
      key: readFileSync(join(fixtureRoot!, 'key.pem')),
      cert: readFileSync(join(fixtureRoot!, 'cert.pem'))
    },
    (request, response) => {
      received.push({
        path: request.url || '',
        cookie: request.headers.cookie || '',
        authorization: request.headers.authorization || ''
      })
      if (request.url === '/redirect') {
        response.writeHead(302, { location: '/redirect-target' }).end()
        return
      }
      response
        .writeHead(200, {
          'content-type': 'application/json',
          'set-cookie': 'response-cookie=should-not-persist; Secure; SameSite=None'
        })
        .end(JSON.stringify({ ok: true }))
    }
  )
  await new Promise<void>((resolve) => origin.listen(0, '127.0.0.1', resolve))
  const originPort = (origin.address() as AddressInfo).port
  const originUrl = `https://127.0.0.1:${originPort}`
  const proxyRequests: string[] = []
  const createProxy = async (label: string) => {
    const proxy = createHttpServer((_request, response) => response.writeHead(400).end())
    proxy.on('connect', (request, client, head) => {
      // This fixture may connect only to its own ephemeral TLS server.
      if (request.url !== `127.0.0.1:${originPort}`) {
        client.destroy()
        return
      }
      proxyRequests.push(label)
      const upstream = connect(originPort, '127.0.0.1', () => {
        client.write('HTTP/1.1 200 Connection Established\r\n\r\n')
        if (head.length) upstream.write(head)
        client.pipe(upstream)
        upstream.pipe(client)
      })
      sockets.add(upstream)
      sockets.add(client as Socket)
      upstream.on('error', () => client.destroy())
      client.on('error', () => upstream.destroy())
      client.on('close', () => upstream.destroy())
    })
    await new Promise<void>((resolve) => proxy.listen(0, '127.0.0.1', resolve))
    return proxy
  }
  const proxyA = await createProxy('A')
  const proxyB = await createProxy('B')
  const accountSession = session.fromPartition('captain-who-account', { cache: false })
  // Only this private test Session trusts the disposable localhost certificate.
  accountSession.setCertificateVerifyProc(({ hostname }, callback) => {
    callback(hostname === '127.0.0.1' ? 0 : -3)
  })
  const fetch = createAccountFetch(async () => accountSession)
  try {
    await session.defaultSession.cookies.set({ url: originUrl, name: 'browser', value: 'isolated' })
    await accountSession.cookies.set({ url: originUrl, name: 'account', value: 'also-omitted' })
    const phaseResults: boolean[] = []
    const snapshots: string[][] = []
    for (const [label, proxy] of [
      ['A', proxyA],
      ['B', proxyB],
      ['direct', null]
    ] as const) {
      await accountSession.setProxy(
        proxy
          ? {
              mode: 'fixed_servers',
              proxyRules: `http://127.0.0.1:${(proxy.address() as AddressInfo).port}`,
              proxyBypassRules: '<-loopback>'
            }
          : { mode: 'direct' }
      )
      const response = await fetch(`${originUrl}/${label}`, {
        headers: { Authorization: 'Bearer synthetic-account-fixture' }
      })
      phaseResults.push((await response.json()).ok === true)
      snapshots.push([...proxyRequests])
    }
    let redirectRejected = false
    try {
      await fetch(`${originUrl}/redirect`)
    } catch {
      redirectRejected = true
    }
    const cookies = await accountSession.cookies.get({ url: originUrl })
    console.log(
      `MYCOPILOT_ACCOUNT_NETWORK_RESULT=${JSON.stringify({
        phaseResults,
        snapshots,
        received,
        redirectRejected,
        responseCookiePersisted: cookies.some((cookie) => cookie.name === 'response-cookie'),
        browserCookiePresent: cookies.some((cookie) => cookie.name === 'browser'),
        isolated: accountSession !== session.defaultSession && !accountSession.isPersistent()
      })}`
    )
  } finally {
    await accountSession.closeAllConnections()
    for (const socket of sockets) socket.destroy()
    for (const server of [origin, proxyA, proxyB]) {
      server.closeAllConnections()
      await new Promise<void>((resolve) => server.close(() => resolve()))
    }
  }
}

run().then(
  () => app.exit(0),
  (error) => {
    console.error(error)
    app.exit(1)
  }
)
