/* eslint-disable @typescript-eslint/no-require-imports -- Set isolated userData before synchronously loading Electron's CommonJS main bundle. */
// Isolated Electron entry for manual/automated login UI smoke tests. Never use real app data.
const { app } = require('electron')
const { isAbsolute, resolve } = require('node:path')
const dataRoot = process.env.CAPTAIN_WHO_AUTH_SMOKE_DATA
if (!dataRoot || !isAbsolute(dataRoot))
  throw new Error('An isolated absolute data directory is required')
app.setPath('userData', dataRoot)
require(resolve(__dirname, '../../out/main/index.js'))
