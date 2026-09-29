import { spawn } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { build } from 'vite'
import { describe, expect, it } from 'vitest'

describe.runIf(process.platform === 'darwin')('scoped Select All native menu', () => {
  it('preserves the app menu while routing host scopes, inputs and guest pages independently', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'mycopilot-scoped-select-all-'))
    try {
      await build({
        build: {
          emptyOutDir: true,
          lib: {
            entry: resolve('src/main/fixtures/scopedSelectAll.electron.ts'),
            fileName: () => 'fixture.mjs',
            formats: ['es']
          },
          minify: false,
          outDir: directory,
          rollupOptions: { external: ['electron', /^node:/u] },
          target: 'node22'
        },
        configFile: false,
        logLevel: 'silent'
      })
      const electron = createRequire(join(process.cwd(), 'package.json'))('electron') as string
      const environment = { ...process.env }
      delete environment.ELECTRON_RUN_AS_NODE
      const result = await new Promise<{ code: number | null; output: string }>(
        (accept, reject) => {
          const child = spawn(
            electron,
            [
              join(directory, 'fixture.mjs'),
              join(directory, 'profile'),
              '-ApplePersistenceIgnoreState',
              'YES'
            ],
            {
              cwd: process.cwd(),
              env: environment,
              stdio: ['ignore', 'pipe', 'pipe']
            }
          )
          let output = ''
          const timer = setTimeout(() => {
            child.kill('SIGKILL')
            reject(new Error(`Selection fixture timed out: ${output}`))
          }, 30_000)
          child.stdout.on('data', (chunk: Buffer) => {
            output += String(chunk)
          })
          child.stderr.on('data', (chunk: Buffer) => {
            output += String(chunk)
          })
          child.once('error', (error) => {
            clearTimeout(timer)
            reject(error)
          })
          child.once('exit', (code) => {
            clearTimeout(timer)
            accept({ code, output })
          })
        }
      )
      expect(result.code, result.output).toBe(0)
      const marker = 'MYCOPILOT_SCOPED_SELECT_ALL='
      const line = result.output.split(/\r?\n/u).find((line) => line.startsWith(marker))
      expect(line, result.output).toBeDefined()
      const observed = JSON.parse(line!.slice(marker.length))
      expect(Object.values(observed.structure)).toEqual(Array(7).fill(true))
      expect(observed.scoped).toBe('PAGE ONE\nPAGE TWO')
      expect(observed.nativeInput).toEqual({ requests: 2, start: 0, end: 15 })
      expect(observed.guestSelection).toContain('GUEST ONE\nGUEST TWO')
      expect(observed.hostRequestsAfterGuest).toBe(2)
      expect(observed.unfocusedFallbackIgnored).toBe(true)
      expect(observed.isolatedProfile).toBe(true)
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  }, 40_000)
})
