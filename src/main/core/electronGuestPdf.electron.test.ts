import { spawn } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { build } from 'vite'
import { describe, expect, it } from 'vitest'

describe.runIf(process.platform === 'darwin')('managed guest PDF Electron fixture', () => {
  it('prints current authenticated form state through Playwright and rejects a real OOPIF without hanging', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'mycopilot-guest-pdf-'))
    try {
      await build({
        build: {
          emptyOutDir: true,
          lib: {
            entry: resolve('src/main/browser/fixtures/electronGuestPdf.electron.ts'),
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
      const requireFromWorkspace = createRequire(join(process.cwd(), 'package.json'))
      const electron = requireFromWorkspace('electron') as string
      const pdfPath = join(directory, 'current-page.pdf')
      const environment = { ...process.env }
      delete environment.ELECTRON_RUN_AS_NODE
      const result = await new Promise<{ code: number | null; output: string }>(
        (accept, reject) => {
          const child = spawn(
            electron,
            [
              join(directory, 'fixture.mjs'),
              pdfPath,
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
            reject(new Error(`PDF fixture timed out: ${output}`))
          }, 35_000)
          child.stdout.on('data', (chunk: Buffer) => {
            output = `${output}${chunk}`.slice(-100_000)
          })
          child.stderr.on('data', (chunk: Buffer) => {
            output = `${output}${chunk}`.slice(-100_000)
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
      const marker = 'MYCOPILOT_GUEST_PDF_RESULT='
      const line = result.output.split(/\r?\n/u).find((value) => value.startsWith(marker))
      expect(line, result.output).toBeDefined()
      const observed = JSON.parse(line!.slice(marker.length)) as Record<string, unknown>
      expect(observed).toMatchObject({
        actualOopif: true,
        currentUrlPreserved: true,
        requestsStable: true,
        oopifRejected: true,
        afterRejectedPrintWorks: true,
        pendingPrintRetained: true,
        recoveredAfterPageClose: true
      })
      expect(['timed_out', 'page_changed', 'cross_process_frame']).toContain(
        observed.dynamicPrintOutcome
      )
      const { getDocument } = await import('pdfjs-dist/legacy/build/pdf.mjs')
      const document = await getDocument({
        data: Uint8Array.from(await readFile(pdfPath)),
        useSystemFonts: true
      }).promise
      try {
        const text: string[] = []
        for (let number = 1; number <= document.numPages; number += 1) {
          const page = await document.getPage(number)
          const content = await page.getTextContent()
          text.push(...content.items.flatMap((item) => ('str' in item ? [item.str] : [])))
        }
        const content = text.join(' ')
        for (const expected of [
          'AUTHENTICATED_ACCOUNT',
          'UNSAVED_INPUT_VALUE',
          'UNSAVED_NOTES_VALUE',
          'LIVE_DYNAMIC_CONTENT',
          'SAME_FRAME_CONTENT'
        ]) {
          expect(content).toContain(expected)
        }
        expect(content).not.toContain('ORIGINAL_INPUT_VALUE')
        expect(content).not.toContain('SCREEN_ONLY_NOT_PRINTED')
      } finally {
        await document.destroy()
      }
    } finally {
      await rm(directory, { force: true, recursive: true })
    }
  }, 45_000)
})
