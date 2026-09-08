import { spawn } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdtemp, rm } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { build } from 'vite'
import { describe, expect, it } from 'vitest'

describe.runIf(process.platform === 'darwin')('managed native popup Electron fixture', () => {
  it('preserves browser login window semantics behind the real surface and network boundaries', async () => {
    const directory = await mkdtemp(join(process.cwd(), '.mycopilot-native-popup-fixture-'))
    try {
      await build({
        build: {
          emptyOutDir: true,
          lib: {
            entry: resolve('src/main/browser/fixtures/browserNativePopup.electron.ts'),
            fileName: () => 'fixture.mjs',
            formats: ['es']
          },
          minify: false,
          outDir: directory,
          rollupOptions: { external: ['electron', 'playwright', /^node:/u] },
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
            reject(new Error(`Native popup fixture timed out: ${output}`))
          }, 50_000)
          child.stdout.on('data', (chunk: Buffer) => {
            output = `${output}${chunk}`.slice(-500_000)
          })
          child.stderr.on('data', (chunk: Buffer) => {
            output = `${output}${chunk}`.slice(-500_000)
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
      const marker = 'MYCOPILOT_NATIVE_POPUP_RESULT='
      const line = result.output.split(/\r?\n/u).find((value) => value.startsWith(marker))
      expect(line, result.output).toBeDefined()
      const observed = JSON.parse(line!.slice(marker.length)) as {
        results: Array<Record<string, unknown>>
        post: Array<{ method: string; body: string }>
        noreferrer: Array<{ referer: string | null }>
        finalSurfaceCount: number
        isolatedProfile: boolean
        noRemoteDebuggingPort: boolean
        rejectedCreation: {
          capacityReturnedNull: boolean
          capacityCreatedNoGuest: boolean
          capacitySentNoRequest: boolean
          injectedFailures: Array<{
            mode: string
            windowCreated: boolean
            windowDestroyedAfterManagerFailure: boolean | null
            destroyedAfterManagerFailure: boolean
          }>
          failureResults: Array<Record<string, unknown>>
          uncaughtExceptions: string[]
        }
      }
      expect(observed.results).toHaveLength(7)
      for (const popup of observed.results) {
        expect(popup).toMatchObject({
          selected: true,
          surfaceRemoved: true,
          openerSurvived: true,
          presentation: {
            width: 520,
            height: 680,
            centeredOnHost: true,
            titleMatchesUrl: true,
            pageTitleCannotOverride: true,
            titleFollowsNavigation: true
          }
        })
        if (['same', 'cross', 'blank'].includes(popup.label as string)) {
          expect(popup).toMatchObject({
            childHasOpener: true,
            bidirectionalMessages: true,
            proxyClosed: true,
            opened: { isNull: false }
          })
        } else if (['noopener', 'noreferrer'].includes(popup.label as string)) {
          expect(popup).toMatchObject({ childHasOpener: false, opened: { isNull: true } })
        } else if (popup.label === 'coop') {
          expect(popup).toMatchObject({
            childHasOpener: false,
            opened: { isNull: false },
            proxyClosed: true
          })
        }
      }
      expect(observed.results.find((popup) => popup.label === 'blank')).toMatchObject({
        opened: { initialOpener: true }
      })
      expect(observed.post).toHaveLength(1)
      expect(observed.post[0]).toMatchObject({
        method: 'POST',
        body: 'state=preserved-original-body'
      })
      expect(observed.noreferrer).toHaveLength(1)
      expect(observed.noreferrer[0].referer).toBeNull()
      expect(observed.finalSurfaceCount).toBe(1)
      expect(observed.isolatedProfile).toBe(true)
      expect(observed.noRemoteDebuggingPort).toBe(true)
      expect(observed.rejectedCreation).toMatchObject({
        capacityReturnedNull: true,
        capacityCreatedNoGuest: true,
        capacitySentNoRequest: true,
        uncaughtExceptions: []
      })
      expect(observed.rejectedCreation.injectedFailures).toMatchObject([
        { mode: 'factory', windowCreated: false, windowDestroyedAfterManagerFailure: null },
        { mode: 'configure', windowCreated: true, windowDestroyedAfterManagerFailure: true }
      ])
      expect(observed.rejectedCreation.failureResults).toEqual([
        {
          mode: 'factory',
          noOrphanContents: true,
          noRequest: true,
          surfaceCount: 1,
          openerSurvived: true
        },
        {
          mode: 'configure',
          noOrphanContents: true,
          noRequest: true,
          surfaceCount: 1,
          openerSurvived: true
        }
      ])
    } finally {
      await rm(directory, { force: true, recursive: true })
    }
  }, 60_000)
})
