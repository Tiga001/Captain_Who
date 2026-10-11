import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { builtinModules, createRequire } from 'node:module'
import { access, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { build } from 'vite'
import react from '@vitejs/plugin-react'
import { describe, expect, it } from 'vitest'

describe.runIf(
  process.platform === 'darwin' && Boolean(process.env.CAPTAIN_CORE_ACCEPTANCE_BINARY)
)('production Renderer hooks through native Electron IPC and the real Core pipe', () => {
  it('keeps admission, approval receipts, Stop receipts and durable terminals distinct', async () => {
    const binary = resolve(process.env.CAPTAIN_CORE_ACCEPTANCE_BINARY!)
    await access(binary)
    const binarySha256 = createHash('sha256')
      .update(await readFile(binary))
      .digest('hex')
    const directory = await mkdtemp(join(tmpdir(), 'captain-acceptance-electron-'))
    let corePid: number | undefined
    try {
      for (const entry of ['backendAcceptance.electron', 'backendAcceptance.preload']) {
        await build({
          configFile: false,
          logLevel: 'silent',
          build: {
            emptyOutDir: false,
            outDir: directory,
            target: 'node22',
            minify: false,
            lib: {
              entry: resolve(`src/main/core/fixtures/${entry}.ts`),
              fileName: () => `${entry}.cjs`,
              formats: ['cjs']
            },
            rollupOptions: { external: ['electron', ...builtinModules, /^node:/u] }
          }
        })
      }
      const rendererRoot = resolve('src/renderer/src/app/__fixtures__')
      const rendererDirectory = join(directory, 'renderer')
      await build({
        configFile: false,
        logLevel: 'silent',
        root: rendererRoot,
        base: './',
        plugins: [react()],
        define: { 'import.meta.env.VITE_CAPTAIN_PERFORMANCE_DIAGNOSTICS': '"1"' },
        resolve: { alias: { '@renderer': resolve('src/renderer/src') } },
        build: {
          outDir: rendererDirectory,
          emptyOutDir: true,
          target: 'esnext',
          minify: false,
          rollupOptions: { input: join(rendererRoot, 'backendAcceptance.html') }
        }
      })
      const electron = createRequire(join(process.cwd(), 'package.json'))('electron') as string
      const environment: NodeJS.ProcessEnv = {
        ...process.env,
        CAPTAIN_PERFORMANCE_DIAGNOSTICS: '1'
      }
      delete environment.ELECTRON_RUN_AS_NODE
      const result = await new Promise<{ code: number | null; output: string }>(
        (accept, reject) => {
          const child = spawn(
            electron,
            [
              join(directory, 'backendAcceptance.electron.cjs'),
              join(directory, 'profile'),
              binary,
              join(rendererDirectory, 'backendAcceptance.html'),
              join(directory, 'backendAcceptance.preload.cjs')
            ],
            { cwd: process.cwd(), env: environment, stdio: ['ignore', 'pipe', 'pipe'] }
          )
          let output = ''
          const timer = setTimeout(() => {
            child.kill('SIGKILL')
            // Only the exact Core PID reported by this fixture is eligible for cleanup.
            if (corePid) {
              try {
                process.kill(corePid, 'SIGKILL')
              } catch {
                /* already exited */
              }
            }
            reject(new Error(`Isolated Renderer acceptance timeout: ${output}`))
          }, 120_000)
          const collect = (chunk: Buffer): void => {
            output += String(chunk)
            const match = [...output.matchAll(/CAPTAIN_ACCEPTANCE_CORE_PID=(\d+)/gu)].at(-1)
            if (match) corePid = Number(match[1])
          }
          child.stdout.on('data', collect)
          child.stderr.on('data', collect)
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
      const marker = 'CAPTAIN_RENDERER_ACCEPTANCE='
      const line = result.output.split(/\r?\n/u).find((line) => line.startsWith(marker))
      const report = line ? JSON.parse(line.slice(marker.length)) : { status: 'missing_report' }
      if (process.env.CAPTAIN_RENDERER_ACCEPTANCE_REPORT) {
        await writeFile(
          process.env.CAPTAIN_RENDERER_ACCEPTANCE_REPORT,
          JSON.stringify({ binarySha256, exitCode: result.code, ...report }, null, 2) + '\n'
        )
        await writeFile(`${process.env.CAPTAIN_RENDERER_ACCEPTANCE_REPORT}.log`, result.output)
      }
      expect(result.code, result.output).toBe(0)
      expect(report.status).toBe('completed')
      expect(report.offeredActions).toBe(50)
      expect(report.completedActions).toBe(50)
      expect(report.failedActions).toBe(0)
      expect(report.unfinishedActions).toBe(0)
      expect(report.verifiedTerminals).toBe(30)
      expect(report.reopenedAssistantMessages).toBe(30)
      expect(report.providerRequests).toBe(40)
      expect(report.terminalEvidence).toHaveLength(30)
      expect(
        report.terminalEvidence.filter((row: { status: string }) => row.status === 'completed')
      ).toHaveLength(20)
      expect(
        report.terminalEvidence.filter((row: { status: string }) => row.status === 'cancelled')
      ).toHaveLength(10)
      expect(report.rpcCounts['agent.startConversationTurn']).toBe(30)
      expect(report.rpcCounts['agent.approveAction']).toBe(10)
      expect(report.rpcCounts['agent.cancelRun']).toBe(10)
      const counts = Object.fromEntries(
        ['submit', 'approve', 'stop'].map((action) => [
          action,
          report.reports.filter((row: { action: string }) => row.action === action).length
        ])
      )
      expect(counts).toEqual({ submit: 30, approve: 10, stop: 10 })
      for (const windows of [report.hostMetrics, report.rendererDiagnostics]) {
        expect(
          windows.reduce(
            (total: number, window: { omittedTraces: number }) => total + window.omittedTraces,
            0
          )
        ).toBe(0)
      }
      const hostTraces = report.hostMetrics.flatMap(
        (window: { traces: Array<{ renderer?: unknown }> }) => window.traces
      )
      const actionTraces = hostTraces.filter(
        (trace: { renderer?: unknown; method: string }) =>
          trace.renderer &&
          ['agent.startConversationTurn', 'agent.approveAction', 'agent.cancelRun'].includes(
            trace.method
          )
      )
      expect(actionTraces).toHaveLength(50)
      const joinedActions = new Set<string>()
      for (const trace of actionTraces) {
        expect(trace.outcome).toBe('completed')
        const { rendererSessionNonce, actionSequence } = trace.renderer
        joinedActions.add(`${rendererSessionNonce}:${actionSequence}`)
        expect(
          report.rendererDiagnostics.some(
            (window: {
              rendererSessionNonce: string
              traces: Array<{ actionSequence: number; stage: string }>
            }) =>
              window.rendererSessionNonce === rendererSessionNonce &&
              window.traces.some(
                (item) => item.actionSequence === actionSequence && item.stage === 'total'
              )
          )
        ).toBe(true)
      }
      expect(joinedActions.size).toBe(50)
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  }, 180_000)
})
