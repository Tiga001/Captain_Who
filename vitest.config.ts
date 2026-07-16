import { resolve } from 'node:path'
import react from '@vitejs/plugin-react'
import { playwright } from '@vitest/browser-playwright'
import { defineConfig } from 'vitest/config'

const gitReviewTests = 'src/renderer/src/features/gitReview/__tests__'

export default defineConfig({
  optimizeDeps: {
    include: ['lucide-react', 'react', 'react-dom', 'react-dom/client', 'react/jsx-dev-runtime']
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
          include: [`${gitReviewTests}/**/*.test.ts`],
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
          include: [`${gitReviewTests}/**/*.browser.test.tsx`],
          name: 'browser'
        }
      }
    ]
  }
})
