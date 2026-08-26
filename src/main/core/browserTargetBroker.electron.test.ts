import { spawn } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { build } from 'vite'
import { describe, expect, it } from 'vitest'

const RESULT_MARKER = 'MYCOPILOT_BROWSER_BROKER_RESULT='

interface FixtureResult {
  ariaSnapshot: boolean
  closedTargetRejected: boolean
  createTargetRejected: boolean
  finalText: string
  focusSpoofContained: boolean
  frameParity: {
    blankEditorText: string
    coordinateEditorText: string
    crossEventTypes: string
    crossInputValue: string
    detachedFrameRejected: boolean
    keyEventTargets: string
    keyEventTypes: string
    navigatedInputValue: string
    nestedInputValue: string
    sameEditorText: string
    sameEventTargets: string
    sameEventTypes: string
    sameInputValue: string
    sameTextareaValue: string
    secondInputValue: string
    shadowEditorText: string
  }
  frameProbe: {
    candidateCount: number
    distinctFramePaths: string[]
    safeProjection: boolean
  }
  isolatedProfile: boolean
  noRemoteDebuggingPort: boolean
  outOfProcessFrameAttached: boolean
  outOfProcessFrameResumed: boolean
  onlySelectedGuest: boolean
  selectedDebuggerDetached: boolean
  snapshot: {
    activeConnections: number
    claimedSurfaces: number
    registeredGuests: number
  }
}

describe.runIf(process.platform === 'darwin')('BrowserTargetBroker Electron fixture', () => {
  it('controls one real webview through Playwright without a debugging port', async () => {
    const outputDirectory = await mkdtemp(join(tmpdir(), 'mycopilot-browser-broker-'))
    try {
      await build({
        build: {
          emptyOutDir: true,
          lib: {
            entry: resolve('src/main/browser/fixtures/browserTargetBroker.electron.ts'),
            fileName: () => 'fixture.mjs',
            formats: ['es']
          },
          minify: false,
          outDir: outputDirectory,
          rollupOptions: { external: ['electron', /^node:/u] },
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
      const execution = await runProcess(
        electronPath,
        [fixturePath, '-ApplePersistenceIgnoreState', 'YES'],
        environment
      )
      expect(execution.exitCode, execution.stderr || execution.stdout).toBe(0)
      const resultLine = execution.stdout
        .split(/\r?\n/u)
        .find((line) => line.startsWith(RESULT_MARKER))
      expect(resultLine, execution.stdout).toBeDefined()
      const result = JSON.parse(resultLine!.slice(RESULT_MARKER.length)) as FixtureResult
      expect(result).toEqual({
        ariaSnapshot: true,
        closedTargetRejected: true,
        createTargetRejected: true,
        finalText: 'applied:hello',
        focusSpoofContained: true,
        frameParity: {
          blankEditorText: '空白你好',
          coordinateEditorText: '坐标你好',
          crossEventTypes:
            'beforeinput,input,beforeinput,input,beforeinput,input,beforeinput,input,keydown,keyup',
          crossInputValue: '跨域你好',
          detachedFrameRejected: true,
          keyEventTargets:
            'key-event-input,key-event-input,key-event-input,key-event-input,key-event-input,key-event-input,key-event-input,key-event-input',
          keyEventTypes: 'keydown,beforeinput,input,keyup,keydown,beforeinput,input,keyup',
          navigatedInputValue: '导航后你好',
          nestedInputValue: '嵌套你好',
          sameEditorText: '编辑器你好',
          sameEventTargets: 'frame-input,frame-input,frame-input,frame-input',
          sameEventTypes: 'beforeinput,input,beforeinput,input',
          sameInputValue: '你好',
          sameTextareaValue: '多行\n你好',
          secondInputValue: '第二帧',
          shadowEditorText: '影子你好'
        },
        frameProbe: {
          candidateCount: 24,
          distinctFramePaths: ['0', '1', '2.0', '3'],
          safeProjection: true
        },
        isolatedProfile: true,
        noRemoteDebuggingPort: true,
        outOfProcessFrameAttached: true,
        outOfProcessFrameResumed: true,
        onlySelectedGuest: true,
        selectedDebuggerDetached: true,
        snapshot: {
          activeConnections: 0,
          claimedSurfaces: 0,
          registeredGuests: 0
        }
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
