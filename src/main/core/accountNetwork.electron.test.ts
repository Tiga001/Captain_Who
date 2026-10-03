import { spawn, spawnSync } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdtemp, rm } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { tmpdir } from 'node:os'
import { build } from 'vite'
import { describe, expect, it } from 'vitest'

describe.runIf(process.platform === 'darwin')('account Electron network fixture', () => {
  it('switches an isolated Session between two local proxies and direct without restarting', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'mycopilot-account-network-'))
    try {
      const certificate = spawnSync(
        'openssl',
        [
          'req',
          '-x509',
          '-newkey',
          'rsa:2048',
          '-nodes',
          '-days',
          '1',
          '-subj',
          '/CN=localhost',
          '-keyout',
          join(directory, 'key.pem'),
          '-out',
          join(directory, 'cert.pem')
        ],
        { stdio: 'ignore' }
      )
      expect(certificate.status).toBe(0)
      await build({
        configFile: false,
        logLevel: 'silent',
        build: {
          emptyOutDir: false,
          outDir: directory,
          target: 'node22',
          minify: false,
          lib: {
            entry: resolve('src/main/auth/fixtures/accountNetwork.electron.ts'),
            fileName: () => 'fixture.mjs',
            formats: ['es']
          },
          rollupOptions: { external: ['electron', /^node:/u] }
        }
      })
      const require = createRequire(join(process.cwd(), 'package.json'))
      const environment: NodeJS.ProcessEnv = {
        ...process.env,
        MYCOPILOT_ACCOUNT_NETWORK_FIXTURE_ROOT: directory
      }
      delete environment.ELECTRON_RUN_AS_NODE
      const output = await new Promise<{ code: number | null; stdout: string; stderr: string }>(
        (resolve, reject) => {
          const child = spawn(require('electron') as string, [join(directory, 'fixture.mjs')], {
            env: environment
          })
          let stdout = ''
          let stderr = ''
          const timeout = setTimeout(() => {
            child.kill('SIGKILL')
            reject(new Error('Account network fixture timed out'))
          }, 40_000)
          child.stdout.on('data', (data) => {
            stdout += data.toString()
          })
          child.stderr.on('data', (data) => {
            stderr += data.toString()
          })
          child.once('error', (error) => {
            clearTimeout(timeout)
            reject(error)
          })
          child.once('exit', (code) => {
            clearTimeout(timeout)
            resolve({ code, stdout, stderr })
          })
        }
      )
      expect(output.code, output.stderr || output.stdout).toBe(0)
      const marker = 'MYCOPILOT_ACCOUNT_NETWORK_RESULT='
      const line = output.stdout.split(/\r?\n/u).find((line) => line.startsWith(marker))
      expect(line, output.stdout).toBeDefined()
      const result = JSON.parse(line!.slice(marker.length))
      expect(result).toMatchObject({
        phaseResults: [true, true, true],
        snapshots: [['A'], ['A', 'B'], ['A', 'B']],
        redirectRejected: true,
        responseCookiePersisted: false,
        browserCookiePresent: false,
        isolated: true
      })
      expect(result.received).toEqual([
        { path: '/A', cookie: '', authorization: 'Bearer synthetic-account-fixture' },
        { path: '/B', cookie: '', authorization: 'Bearer synthetic-account-fixture' },
        { path: '/direct', cookie: '', authorization: 'Bearer synthetic-account-fixture' },
        { path: '/redirect', cookie: '', authorization: '' }
      ])
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  }, 60_000)
})
