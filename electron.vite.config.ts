// Electron/Vite build configuration.
import { resolve } from 'path'
import { defineConfig } from 'electron-vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  main: {
    build: {
      rollupOptions: {
        input: {
          index: resolve('src/main/index.ts'),
          'terminal-service': resolve('src/main/terminal/terminal-service.ts')
        }
      }
    }
  },
  preload: {},
  renderer: {
    optimizeDeps: {
      // Worker language modules use deep entrypoints. Pre-bundle them before the
      // dev renderer starts so first-time file previews never trigger a reload.
      include: [
        '@pierre/trees',
        '@pierre/trees/react',
        '@shikijs/langs/*',
        'shiki/core',
        'shiki/engine/javascript'
      ]
    },
    resolve: {
      alias: {
        '@renderer': resolve('src/renderer/src')
      }
    },
    plugins: [react()],
    worker: {
      // Shiki grammars are loaded as split ESM chunks inside the syntax-highlighting worker.
      format: 'es'
    }
  }
})
