import { spawn } from 'node:child_process'
import { createRequire } from 'node:module'
import { access, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { build } from 'vite'
import { describe, expect, it } from 'vitest'

describe.runIf(process.platform === 'darwin')(
  'narrow file proof through the real Electron Host/Core pipe',
  () => {
    it('preserves the same proof and deletion while measuring identical sequential reads', async () => {
      // Building is deliberately external: the coordinated backend validation owns one target.
      const binary = resolve(process.env.CAPTAIN_CORE_TEST_BINARY ?? 'target/debug/core-server')
      await access(binary)
      const directory = await mkdtemp(join(tmpdir(), 'captain-read-context-electron-'))
      try {
        await build({
          build: {
            emptyOutDir: true,
            lib: {
              entry: resolve('src/main/core/fixtures/readToolContext.electron.ts'),
              fileName: () => 'fixture.cjs',
              formats: ['cjs']
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
              [join(directory, 'fixture.cjs'), join(directory, 'profile'), binary],
              {
                cwd: process.cwd(),
                env: environment,
                stdio: ['ignore', 'pipe', 'pipe']
              }
            )
            let output = ''
            const timer = setTimeout(() => {
              child.kill('SIGKILL')
              reject(new Error(`Read context fixture timeout: ${output}`))
            }, 45_000)
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
            child.once('close', (code) => {
              clearTimeout(timer)
              accept({ code, output })
            })
          }
        )
        expect(result.code, result.output).toBe(0)
        const marker = 'MYCOPILOT_READ_TOOL_CONTEXT='
        const line = result.output.split(/\r?\n/u).find((line) => line.startsWith(marker))
        expect(line, result.output).toBeDefined()
        const report = JSON.parse(line!.slice(marker.length))
        expect(report.proofEqual).toBe(true)
        expect(report.deletionObserved).toBe(true)
        expect(report.fullMetrics).toHaveLength(1)
        expect(report.narrowMetrics).toHaveLength(1)
        const full = report.fullMetrics[0]
        const narrow = report.narrowMetrics[0]
        expect(full.outcomes['completed:storage.loadConversation']).toBe(report.iterations)
        expect(narrow.outcomes['completed:storage.loadReadToolFileContext']).toBe(report.iterations)
        expect(narrow.samples['response.parse:storage.loadReadToolFileContext'].bytes).toBeLessThan(
          full.samples['response.parse:storage.loadConversation'].bytes / 100
        )
        expect(full.pendingPeak).toBe(1)
        expect(narrow.pendingPeak).toBe(1)
        if (process.env.CAPTAIN_HOST_READ_REPORT) {
          await writeFile(
            process.env.CAPTAIN_HOST_READ_REPORT,
            JSON.stringify(report, null, 2) + '\n'
          )
        }
      } finally {
        await rm(directory, { recursive: true, force: true })
      }
    }, 60_000)
  }
)
