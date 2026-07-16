import { resolve } from 'node:path'
import react from '@vitejs/plugin-react'
import { playwright } from '@vitest/browser-playwright'
import { defineConfig } from 'vitest/config'

const gitReviewTests = 'src/renderer/src/features/gitReview/__tests__'
const rightSidebarTests = 'src/renderer/src/features/rightSidebar/__tests__'

export default defineConfig({
  optimizeDeps: {
    // Keep Worker language chunks lazy in browser tests. Auto-discovery would
    // eagerly optimize every literal @shikijs/langs import and reload the test
    // page while an assertion is running.
    noDiscovery: true,
    exclude: ['@shikijs/langs'],
    include: [
      'lucide-react',
      'react',
      'react-dom',
      'react-dom/client',
      'react/jsx-dev-runtime',
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
          include: [`${gitReviewTests}/**/*.test.ts`, `${rightSidebarTests}/**/*.test.ts`],
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
            `${gitReviewTests}/**/*.browser.test.tsx`,
            `${rightSidebarTests}/**/*.browser.test.tsx`
          ],
          name: 'browser'
        }
      }
    ]
  }
})
