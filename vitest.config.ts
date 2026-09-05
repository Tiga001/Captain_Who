import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import react from '@vitejs/plugin-react'
import { playwright } from '@vitest/browser-playwright'
import { defineConfig } from 'vitest/config'
import { vitestProjectFileRules } from './scripts/vitest-project-rules.mjs'
const officeRendererManifest = JSON.parse(
  readFileSync(resolve('resources/office-renderer-manifest.json'), 'utf8')
) as { targets: Record<string, { executable: string }> }
const officeRendererTarget = officeRendererManifest.targets[`${process.platform}-${process.arch}`]

if (!officeRendererTarget) {
  throw new Error(`Office renderer is unavailable for ${process.platform}-${process.arch}`)
}

const browserTestExecutable = resolve(
  '.cache/office-renderer/current',
  officeRendererTarget.executable
)

export default defineConfig({
  optimizeDeps: {
    // Browser tests import Worker language chunks through many deep entrypoints.
    // Pre-bundle them before the run so Vite never reloads an active test page.
    noDiscovery: true,
    include: [
      '@shikijs/langs/*',
      'lucide-react',
      '@pierre/trees',
      '@pierre/trees/react',
      'react',
      'react-dom',
      'react-dom/client',
      'react-markdown',
      'react/jsx-dev-runtime',
      'remark-gfm',
      'shiki/core',
      'shiki/engine/javascript'
    ]
  },
  plugins: [react()],
  resolve: {
    alias: {
      '@renderer': resolve('src/renderer/src')
    }
  },
  test: {
    projects: [
      {
        test: {
          environment: 'node',
          ...vitestProjectFileRules.unit,
          name: 'unit'
        }
      },
      {
        test: {
          browser: {
            enabled: true,
            headless: true,
            instances: [{ browser: 'chromium' }],
            provider: playwright({ launchOptions: { executablePath: browserTestExecutable } })
          },
          ...vitestProjectFileRules.browser,
          name: 'browser'
        }
      },
      {
        test: {
          environment: 'node',
          fileParallelism: false,
          ...vitestProjectFileRules['electron-fixtures'],
          name: 'electron-fixtures'
        }
      },
      {
        test: {
          environment: 'node',
          fileParallelism: false,
          ...vitestProjectFileRules['managed-playwright-e2e'],
          name: 'managed-playwright-e2e'
        }
      },
      {
        test: {
          environment: 'node',
          fileParallelism: false,
          ...vitestProjectFileRules['human-interaction-core-e2e'],
          name: 'human-interaction-core-e2e'
        }
      },
      {
        test: {
          environment: 'node',
          fileParallelism: false,
          ...vitestProjectFileRules['automation-core-e2e'],
          name: 'automation-core-e2e'
        }
      }
    ]
  }
})
