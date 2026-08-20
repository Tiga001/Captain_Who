import { spawn } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { build } from 'vite'
import { describe, expect, it } from 'vitest'

const RESULT_MARKER = 'MYCOPILOT_BROWSER_SURFACE_RESULT='

interface FixtureResult {
  ariaSnapshot: boolean
  closeCommands: number
  createCommands: number
  concurrentSingleFlight: boolean
  ensureCommands: number
  finalText: string
  hiddenTitle: string
  hiddenText: string
  isolatedProfile: boolean
  inputValueAfterFill: string
  mainWindowAliveAfterClose: boolean
  minimizedTitle: string
  multiTab: {
    createdSurfaceId: string
    popupKeptOpenerActive: boolean
    popupTitle: string
    selectedRetainedText: string
    secondTitle: string
    tabsAfterBackgroundClose: number
    tabsAfterCreate: number
    tabsAfterPopup: number
  }
  noRemoteDebuggingPort: boolean
  newRendererReadyAcks: number
  oldTargetRejected: boolean
  pageCount: number
  pressKeyObserved: boolean
  replacementIsInert: boolean
  retainedAfterDetach: boolean
  retainedText: string
  rendererReadyAcks: number
  richText: string
  snapshot: {
    activeConnections: number
    claimedSurfaces: number
    registeredGuests: number
  }
  waitCompleted: boolean
}

describe.runIf(process.platform === 'darwin')('BrowserSurfaceManager Electron fixture', () => {
  it('keeps one exact right-sidebar guest alive across visibility and automation lifecycle', async () => {
    // Keep the temporary bundle below the workspace so ESM can resolve the pinned Playwright
    // runtime alias from this repository's node_modules. The directory is always removed below.
    const outputDirectory = await mkdtemp(
      join(process.cwd(), '.mycopilot-browser-surface-fixture-')
    )
    try {
      await build({
        build: {
          emptyOutDir: true,
          lib: {
            entry: resolve('src/main/browser/fixtures/browserSurfaceManager.electron.ts'),
            fileName: () => 'fixture.mjs',
            formats: ['es']
          },
          minify: false,
          outDir: outputDirectory,
          rollupOptions: {
            external: ['electron', 'playwright', /^node:/u]
          },
          target: 'node22'
        },
        configFile: false,
        logLevel: 'silent'
      })

      const fixturePath = join(outputDirectory, 'fixture.mjs')
      await readFile(fixturePath, 'utf8')
      const requireFromWorkspace = createRequire(join(process.cwd(), 'package.json'))
      const electronPath = requireFromWorkspace('electron') as string
      const environment = { ...process.env }
      delete environment.ELECTRON_RUN_AS_NODE
      const execution = await runProcess(electronPath, [fixturePath], environment)
      expect(execution.exitCode, execution.stderr || execution.stdout).toBe(0)
      const resultLine = execution.stdout
        .split(/\r?\n/u)
        .find((line) => line.startsWith(RESULT_MARKER))
      expect(resultLine, execution.stdout).toBeDefined()
      const result = JSON.parse(resultLine!.slice(RESULT_MARKER.length)) as FixtureResult
      expect(result).toEqual({
        ariaSnapshot: true,
        closeCommands: 3,
        createCommands: 2,
        concurrentSingleFlight: true,
        ensureCommands: 2,
        finalText: 'applied:hello',
        hiddenTitle: 'Browser Surface Fixture',
        hiddenText: 'applied:hidden',
        isolatedProfile: true,
        inputValueAfterFill: 'hidden',
        mainWindowAliveAfterClose: true,
        minimizedTitle: 'Browser Surface Fixture',
        multiTab: {
          createdSurfaceId: 'right-sidebar-browser-electron-fixture-second',
          popupKeptOpenerActive: true,
          popupTitle: 'Browser Surface Fixture',
          selectedRetainedText: 'applied:hidden',
          secondTitle: 'Browser Surface Fixture',
          tabsAfterBackgroundClose: 1,
          tabsAfterCreate: 2,
          tabsAfterPopup: 2
        },
        noRemoteDebuggingPort: true,
        newRendererReadyAcks: 4,
        oldTargetRejected: true,
        pageCount: 1,
        pressKeyObserved: true,
        replacementIsInert: true,
        retainedAfterDetach: true,
        retainedText: 'applied:hidden',
        rendererReadyAcks: 4,
        richText: 'rich text',
        snapshot: {
          activeConnections: 0,
          claimedSurfaces: 0,
          registeredGuests: 0
        },
        waitCompleted: true
      })
    } finally {
      await rm(outputDirectory, { force: true, recursive: true })
    }
  }, 60_000)
})

async function runProcess(
  executable: string,
  arguments_: string[],
  environment: NodeJS.ProcessEnv
): Promise<{ exitCode: number | null; stderr: string; stdout: string }> {
  return await new Promise((resolve, reject) => {
    const child = spawn(executable, arguments_, {
      cwd: process.cwd(),
      env: environment,
      stdio: ['ignore', 'pipe', 'pipe']
    })
    let stderr = ''
    let stdout = ''
    const timeout = setTimeout(() => {
      child.kill('SIGKILL')
      reject(new Error(`Electron fixture timed out\nstdout:\n${stdout}\nstderr:\n${stderr}`))
    }, 45_000)
    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', (chunk: string) => {
      stdout = `${stdout}${chunk}`.slice(-1_000_000)
    })
    child.stderr.on('data', (chunk: string) => {
      stderr = `${stderr}${chunk}`.slice(-1_000_000)
    })
    child.once('error', (error) => {
      clearTimeout(timeout)
      reject(error)
    })
    child.once('exit', (exitCode) => {
      clearTimeout(timeout)
      resolve({ exitCode, stderr, stdout })
    })
  })
}
