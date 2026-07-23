import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import test from 'node:test'

import { createDevelopmentIconFileName } from './prepare-dev-electron-icon.mjs'

test('development Electron bundle icon names are stable and content-addressed', () => {
  const first = createDevelopmentIconFileName(Buffer.from('first icon'))
  const repeated = createDevelopmentIconFileName(Buffer.from('first icon'))
  const second = createDevelopmentIconFileName(Buffer.from('second icon'))

  assert.match(first, /^mycopilot-dev-[a-f0-9]{12}\.icns$/)
  assert.equal(repeated, first)
  assert.notEqual(second, first)
})

test('the development command brands Electron before electron-vite starts', async () => {
  const packageJson = JSON.parse(await readFile(resolve('package.json'), 'utf8'))
  const developmentCommand = packageJson.scripts.dev
  const brandingIndex = developmentCommand.indexOf('pnpm prepare:dev-electron-icon')
  const electronViteIndex = developmentCommand.indexOf('electron-vite dev')

  assert.ok(brandingIndex >= 0)
  assert.ok(electronViteIndex > brandingIndex)
})
