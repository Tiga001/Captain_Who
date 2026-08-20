import { spawn } from 'node:child_process'
import { builtinModules, createRequire } from 'node:module'
import { access, mkdtemp, readFile, readdir, rm } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { tmpdir } from 'node:os'
import { build } from 'vite'
import { describe, expect, it } from 'vitest'

describe.runIf(process.platform === 'darwin')(
  'managed Playwright cross-language Electron E2E',
  () => {
    it('routes the Rust Manager and Catalog through the official host to one local guest', async () => {
      // Keep the bundle under the workspace so its externalized, exactly pinned production packages
      // resolve through this repository's node_modules. The directory is always removed below.
      const outputDirectory = await mkdtemp(
        join(process.cwd(), '.mycopilot-playwright-bridge-e2e-')
      )
      const cwdArtifact = join(process.cwd(), '.playwright-mcp')
      expect(await exists(cwdArtifact)).toBe(false)
      const temporaryOutputsBefore = await managedOutputDirectories()
      const profileDirectoriesBefore = await managedProfileDirectories()
      try {
        await build({
          build: {
            emptyOutDir: true,
            lib: {
              entry: resolve('src/main/browser/fixtures/managedPlaywrightBridge.electron.ts'),
              fileName: () => 'fixture.mjs',
              formats: ['es']
            },
            minify: false,
            outDir: outputDirectory,
            rollupOptions: {
              external: [
                'electron',
                'playwright',
                '@playwright/mcp',
                /^@modelcontextprotocol\/sdk(?:\/.*)?$/u,
                ...builtinModules,
                ...builtinModules.map((moduleName) => `node:${moduleName}`)
              ]
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
          MYCOPILOT_MANAGED_PLAYWRIGHT_ELECTRON: electronPath,
          MYCOPILOT_MANAGED_PLAYWRIGHT_FIXTURE: fixturePath
        }
        delete environment.ELECTRON_RUN_AS_NODE

        const execution = await runProcess(
          'cargo',
          [
            'test',
            '-p',
            'mycopilot-core-server',
            'managed_playwright_official_electron_e2e',
            '--',
            '--ignored',
            '--nocapture'
          ],
          environment
        )
        expect(execution.exitCode, execution.stderr || execution.stdout).toBe(0)
        expect(execution.stdout).toContain('managed_playwright_official_electron_e2e ... ok')
        expect(await exists(cwdArtifact)).toBe(false)
        expect(await managedOutputDirectories()).toEqual(temporaryOutputsBefore)
      } finally {
        await rm(outputDirectory, { force: true, recursive: true })
        const currentProfiles = await managedProfileDirectories()
        await Promise.all(
          currentProfiles
            .filter((name) => !profileDirectoriesBefore.includes(name))
            .map((name) => rm(join(tmpdir(), name), { force: true, recursive: true }))
        )
        expect(
          (await managedProfileDirectories()).every((name) =>
            profileDirectoriesBefore.includes(name)
          )
        ).toBe(true)
      }
    }, 180_000)
  }
)

async function exists(path: string): Promise<boolean> {
  try {
    await access(path)
    return true
  } catch {
    return false
  }
}

async function managedOutputDirectories(): Promise<string[]> {
  return (await readdir(tmpdir()))
    .filter((name) => name.startsWith('mycopilot-playwright-mcp-'))
    .sort()
}

async function managedProfileDirectories(): Promise<string[]> {
  return (await readdir(tmpdir()))
    .filter((name) => name.startsWith('mycopilot-managed-playwright-profile-'))
    .sort()
}

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
      reject(new Error(`managed Playwright E2E timed out\nstdout:\n${stdout}\nstderr:\n${stderr}`))
    }, 170_000)
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
