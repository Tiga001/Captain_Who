import { resolve } from 'node:path'
import react from '@vitejs/plugin-react'
import { playwright } from '@vitest/browser-playwright'
import { defineConfig } from 'vitest/config'

const gitReviewTests = 'src/renderer/src/features/gitReview/__tests__'
const rightSidebarTests = 'src/renderer/src/features/rightSidebar/__tests__'
const filesTests = 'src/renderer/src/features/files/__tests__'
const workspaceFilesTests = 'src/main/workspaceFiles'
const coreMainTests = 'src/main/core'
const terminalMainTests = 'src/main/terminal'
const terminalPreloadTests = 'src/preload'
const terminalRendererTests = 'src/renderer/src/features/terminal/__tests__'
const skillsTests = 'src/renderer/src/features/skills/__tests__'
const appTests = 'src/renderer/src/app/__tests__'
const chatTests = 'src/renderer/src/features/chat/__tests__'
const protocolTests = 'packages/protocol/src'

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
          include: [
            `${gitReviewTests}/**/*.test.ts`,
            `${rightSidebarTests}/**/*.test.ts`,
            `${coreMainTests}/**/*.test.ts`,
            `${workspaceFilesTests}/**/*.test.ts`,
            `${terminalMainTests}/**/*.test.ts`,
            `${terminalPreloadTests}/**/*.test.ts`,
            `${terminalRendererTests}/**/*.test.ts`,
            `${skillsTests}/**/*.test.ts`,
            `${appTests}/**/*.test.ts`,
            `${chatTests}/**/*.test.ts`,
            `${protocolTests}/**/*.test.ts`
          ],
          name: 'unit'
        }
      },
      {
        test: {
          browser: {
            enabled: true,
            headless: true,
            instances: [{ browser: 'chromium' }],
            provider: playwright()
          },
          include: [
            `${appTests}/**/*.browser.test.tsx`,
            `${chatTests}/**/*.browser.test.tsx`,
            `${skillsTests}/**/*.browser.test.tsx`,
            `${gitReviewTests}/**/*.browser.test.tsx`,
            `${rightSidebarTests}/**/*.browser.test.tsx`,
            `${filesTests}/**/*.browser.test.tsx`
          ],
          name: 'browser'
        }
      }
    ]
  }
})
