/* eslint-disable @typescript-eslint/no-require-imports -- Configure the isolated Electron process before loading the real Main bundle. */
/* eslint-disable @typescript-eslint/explicit-function-return-type -- Electron loads these runtime-validated JavaScript helpers directly. */
const { app, net } = require('electron')
const { isAbsolute, resolve } = require('node:path')

const dataRoot = process.env.CAPTAIN_WHO_LOCAL_ACCOUNT_SMOKE_DATA
if (!dataRoot || !isAbsolute(dataRoot))
  throw new Error('An isolated absolute data directory is required')
app.setPath('userData', dataRoot)

if (process.env.CAPTAIN_WHO_LOCAL_ACCOUNT_TRACE_CORE === '1') {
  const childProcess = require('node:child_process')
  const { appendFileSync } = require('node:fs')
  const originalSpawn = childProcess.spawn
  const trace = (value) =>
    appendFileSync(resolve(dataRoot, 'core-transport.jsonl'), `${JSON.stringify(value)}\n`)
  childProcess.spawn = function (command, args, options) {
    const child = originalSpawn.call(this, command, args, options)
    if (command !== 'cargo' && !String(command).endsWith('core-server')) return child
    trace({ event: 'spawn', command, args, cwd: options?.cwd, pid: child.pid })
    const originalWrite = child.stdin.write
    child.stdin.write = function (...writeArgs) {
      for (const line of String(writeArgs[0]).split('\n').filter(Boolean)) {
        try {
          const message = JSON.parse(line)
          trace({ event: 'stdin', id: message.id, method: message.method, bytes: line.length })
        } catch {
          trace({ event: 'stdin', bytes: line.length })
        }
      }
      return originalWrite.apply(this, writeArgs)
    }
    let buffer = ''
    child.stdout.on('data', (chunk) => {
      buffer += String(chunk)
      const lines = buffer.split('\n')
      buffer = lines.pop()
      for (const line of lines.filter(Boolean)) {
        try {
          const message = JSON.parse(line)
          trace({
            event: 'stdout',
            id: message.id,
            method: message.method,
            resultKeys: message.result ? Object.keys(message.result) : undefined,
            errorCode: message.error?.code
          })
        } catch {
          trace({ event: 'stdout', bytes: line.length })
        }
      }
    })
    child.on('exit', (code, signal) => trace({ event: 'exit', code, signal }))
    return child
  }
}

// Fail closed before the production bundle initializes authentication or creates a window.
// Loopback remains available to Electron/Playwright; no remote service can be contacted.
global.__captainWhoLocalAccountSmoke = { blocked: [] }
function remoteHost(target) {
  try {
    const url = new URL(typeof target === 'string' ? target : (target.url ?? target.href))
    if (!['http:', 'https:', 'ws:', 'wss:'].includes(url.protocol)) return null
    return ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname) ? null : url.hostname
  } catch {
    const hostname = target?.hostname ?? target?.host
    if (hostname && ['localhost', '127.0.0.1', '::1'].includes(hostname)) return null
    return hostname ?? 'unknown'
  }
}
function rejectRemote(source, target) {
  const hostname = remoteHost(target)
  if (!hostname) return
  global.__captainWhoLocalAccountSmoke.blocked.push({ source, hostname })
  throw new Error('Offline smoke test blocks remote network access')
}
const originalFetch = global.fetch
global.fetch = async (...args) => {
  rejectRemote('fetch', args[0])
  return originalFetch(...args)
}
for (const name of ['http', 'https']) {
  const module = require(`node:${name}`)
  for (const method of ['request', 'get']) {
    const original = module[method]
    module[method] = function (...args) {
      rejectRemote(`${name}.${method}`, args[0])
      return original.apply(this, args)
    }
  }
}
for (const method of ['fetch', 'request']) {
  const original = net[method]
  net[method] = function (...args) {
    rejectRemote(`electron.net.${method}`, args[0])
    return original.apply(this, args)
  }
}
app.on('session-created', (session) => {
  session.webRequest.onBeforeRequest((details, callback) => {
    const hostname = remoteHost(details.url)
    if (hostname) {
      global.__captainWhoLocalAccountSmoke.blocked.push({ source: 'browser', hostname })
      callback({ cancel: true })
    } else callback({ cancel: false })
  })
})

require(resolve(__dirname, '../../out/main/index.js'))
