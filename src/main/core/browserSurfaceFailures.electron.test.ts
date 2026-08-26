import { spawn } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { tmpdir } from 'node:os'
import { build } from 'vite'
import { describe, expect, it } from 'vitest'

const RESULT_MARKER = 'MYCOPILOT_BROWSER_FAILURE_RESULT='

interface FixtureResult {
  addressBarHistory: {
    backReturnedToFailure: boolean
    forwardReturnedToC: boolean
  }
  crash: {
    diagnosticRedacted: boolean
    generationStable: boolean
    heading: string
    kind: string
    logicalUrl: string
    recovered: boolean
  }
  forgedInternalUrl: {
    authorized: boolean
    rejected: boolean
    statePreserved: boolean
  }
  iframeFailureIgnored: boolean
  offline: {
    cdpHistorySanitized: boolean
    failedUrl: string
    forwardRestoredFailure: boolean
    heading: string
    keyboardFocus: boolean
    kind: string
    layout: LayoutResult
    logicalUrl: string
    physicalInternal: boolean
    physicalOmitsSecret: boolean
    pixelVariation: boolean
    remainedFailedUntilRetry: boolean
    retryRecovered: boolean
    targetStable: boolean
    targetUrlIsLogical: boolean
  }
  physicalHistory: {
    currentEntryIsInternal: boolean
    internalCount: number
    retainedEntriesAuthorized: boolean
  }
  refused: {
    heading: string
    kind: string
    layout: LayoutResult
    logicalUrl: string
    pixelVariation: boolean
  }
  surfaceId: string
}

interface LayoutResult {
  actionVisible: boolean
  height: number
  headingVisible: boolean
  horizontalOverflow: boolean
  verticalOverflow: boolean
  width: number
}

describe.runIf(process.platform === 'darwin')('BrowserSurface failure Electron fixture', () => {
  it('renders, isolates, navigates, and recovers real internal failure documents', async () => {
    const outputDirectory = await mkdtemp(
      join(process.cwd(), '.mycopilot-browser-failure-fixture-')
    )
    const configuredVisualDirectory = process.env.MYCOPILOT_BROWSER_VISUAL_DIR
    const visualDirectory =
      configuredVisualDirectory ??
      (await mkdtemp(join(tmpdir(), 'mycopilot-browser-failure-visual-')))
    try {
      await build({
        build: {
          emptyOutDir: true,
          lib: {
            entry: resolve('src/main/browser/fixtures/browserSurfaceFailures.electron.ts'),
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
      const environment: NodeJS.ProcessEnv = {
        ...process.env,
        MYCOPILOT_BROWSER_VISUAL_DIR: visualDirectory
      }
      delete environment.ELECTRON_RUN_AS_NODE
      const execution = await runProcess(
        electronPath,
        [fixturePath, '-ApplePersistenceIgnoreState', 'YES'],
        environment
      )
      expect(execution.exitCode, JSON.stringify(execution, null, 2)).toBe(0)
      const resultLine = execution.stdout
        .split(/\r?\n/u)
        .find((line) => line.startsWith(RESULT_MARKER))
      expect(resultLine, execution.stdout).toBeDefined()
      const result = JSON.parse(resultLine!.slice(RESULT_MARKER.length)) as FixtureResult

      expect(result.surfaceId).toBe('right-sidebar-browser-electron-failure-fixture')
      expect(result.offline).toMatchObject({
        cdpHistorySanitized: true,
        forwardRestoredFailure: true,
        heading: '无法访问此站点',
        keyboardFocus: true,
        kind: 'offline',
        physicalInternal: true,
        physicalOmitsSecret: true,
        pixelVariation: true,
        remainedFailedUntilRetry: true,
        retryRecovered: true,
        targetStable: true,
        targetUrlIsLogical: true
      })
      expect(result.offline.failedUrl).toBe(result.offline.logicalUrl)
      expect(result.offline.layout).toEqual({
        actionVisible: true,
        height: expect.any(Number),
        headingVisible: true,
        horizontalOverflow: false,
        verticalOverflow: false,
        width: expect.any(Number)
      })
      expect(result.offline.layout.height).toBeGreaterThan(600)
      expect(result.offline.layout.width).toBeLessThan(400)
      expect(result.addressBarHistory).toEqual({
        backReturnedToFailure: true,
        forwardReturnedToC: true
      })
      expect(result.iframeFailureIgnored).toBe(true)
      expect(result.refused).toMatchObject({
        heading: 'This site cannot be reached',
        kind: 'connection_refused',
        pixelVariation: true
      })
      expect(result.refused.logicalUrl).toContain('/refused?token=private')
      expect(result.refused.layout.horizontalOverflow).toBe(false)
      expect(result.refused.layout.verticalOverflow).toBe(false)
      expect(result.refused.layout.height).toBeGreaterThan(600)
      expect(result.refused.layout.width).toBeGreaterThan(700)
      expect(result.forgedInternalUrl).toEqual({
        authorized: false,
        rejected: true,
        statePreserved: true
      })
      expect(result.crash).toMatchObject({
        diagnosticRedacted: true,
        generationStable: true,
        heading: 'The page crashed',
        kind: 'renderer_crashed',
        recovered: true
      })
      expect(result.physicalHistory).toEqual({
        currentEntryIsInternal: false,
        internalCount: 2,
        retainedEntriesAuthorized: true
      })
      expect(
        (await stat(join(visualDirectory, 'offline-zh-light-narrow.png'))).size
      ).toBeGreaterThan(1_000)
      expect((await stat(join(visualDirectory, 'refused-en-dark-wide.png'))).size).toBeGreaterThan(
        1_000
      )
    } finally {
      await rm(outputDirectory, { force: true, recursive: true })
      if (!configuredVisualDirectory) {
        await rm(visualDirectory, { force: true, recursive: true })
      }
    }
  }, 90_000)
})

async function runProcess(
  executable: string,
  arguments_: string[],
  environment: NodeJS.ProcessEnv
): Promise<{
  exitCode: number | null
  signal: NodeJS.Signals | null
  stderr: string
  stdout: string
}> {
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
    }, 80_000)
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
    child.once('exit', (exitCode, signal) => {
      clearTimeout(timeout)
      resolve({ exitCode, signal, stderr, stdout })
    })
  })
}
