import { defineConfig } from 'eslint/config'
import tseslint from '@electron-toolkit/eslint-config-ts'
import eslintConfigPrettier from '@electron-toolkit/eslint-config-prettier'
import eslintPluginReact from 'eslint-plugin-react'
import eslintPluginReactHooks from 'eslint-plugin-react-hooks'
import eslintPluginReactRefresh from 'eslint-plugin-react-refresh'

export default defineConfig(
  { ignores: ['**/.cache', '**/node_modules', '**/dist', '**/out', '**/target'] },
  tseslint.configs.recommended,
  eslintPluginReact.configs.flat.recommended,
  eslintPluginReact.configs.flat['jsx-runtime'],
  {
    settings: {
      react: {
        version: 'detect'
      }
    }
  },
  {
    files: ['**/*.{ts,tsx}'],
    plugins: {
      'react-hooks': eslintPluginReactHooks,
      'react-refresh': eslintPluginReactRefresh
    },
    rules: {
      ...eslintPluginReactHooks.configs.recommended.rules,
      ...eslintPluginReactRefresh.configs.vite.rules,
      // TypeScript already checks inferred return types; requiring annotations on every local
      // callback made the lint command unusable without adding meaningful safety.
      '@typescript-eslint/explicit-function-return-type': 'off',
      // The renderer intentionally resets local UI state when props or active panels change.
      'react-hooks/set-state-in-effect': 'off',
      // Context providers and their hooks are intentionally colocated in this desktop renderer.
      'react-refresh/only-export-components': 'off'
    }
  },
  {
    files: ['src/renderer/src/features/**/*.{ts,tsx}'],
    rules: {
      'no-restricted-imports': [
        'error',
        {
          patterns: [
            {
              group: ['**/app/**'],
              message:
                'Feature modules must not depend on the application composition layer. Move shared domain code into a feature or shared module.'
            }
          ]
        }
      ]
    }
  },
  {
    files: ['src/renderer/src/components/**/*.{ts,tsx}'],
    rules: {
      'no-restricted-imports': [
        'error',
        {
          patterns: [
            {
              group: ['**/app/**', '**/features/**'],
              message:
                'Shared UI components must remain independent of application and feature modules.'
            }
          ]
        }
      ]
    }
  },
  {
    files: ['src/main/**/*.ts', 'src/preload/**/*.ts'],
    ignores: ['**/*.test.ts'],
    rules: {
      'no-restricted-syntax': [
        'error',
        {
          selector: 'Literal[value=/^host:/]',
          message: 'Host IPC channel names must come from HOST_CHANNELS in @mycopilot/host-api.'
        }
      ]
    }
  },
  eslintConfigPrettier
)
