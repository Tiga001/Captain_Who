/**
 * Brands Electron's macOS development bundle before it reaches the Dock.
 *
 * `app.dock.setIcon()` runs only after Electron has launched, so macOS otherwise shows the
 * stock Electron icon briefly during startup and shutdown. Production bundles already use
 * build/icon.icns; this script gives the raw Electron.app used by `electron-vite dev` the same
 * bundle-level icon. Other platforms intentionally remain untouched.
 */
/* eslint-disable @typescript-eslint/explicit-function-return-type -- Node.js build helper uses runtime-validated JavaScript. */

import { createHash } from 'node:crypto'
import { copyFile, readFile, stat, utimes } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawnSync } from 'node:child_process'

const require = createRequire(import.meta.url)
const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)))

export function createDevelopmentIconFileName(iconBytes) {
  const digest = createHash('sha256').update(iconBytes).digest('hex').slice(0, 12)
  return `mycopilot-dev-${digest}.icns`
}

function readBundleIconFile(infoPlistPath) {
  const result = spawnSync(
    '/usr/libexec/PlistBuddy',
    ['-c', 'Print :CFBundleIconFile', infoPlistPath],
    {
      encoding: 'utf8',
      shell: false
    }
  )
  if (result.status !== 0) {
    throw new Error(`Unable to read Electron development bundle icon: ${result.stderr.trim()}`)
  }
  return result.stdout.trim()
}

function writeBundleIconFile(infoPlistPath, iconFileName) {
  const result = spawnSync(
    '/usr/libexec/PlistBuddy',
    ['-c', `Set :CFBundleIconFile ${iconFileName}`, infoPlistPath],
    {
      encoding: 'utf8',
      shell: false
    }
  )
  if (result.status !== 0) {
    throw new Error(`Unable to brand Electron development bundle: ${result.stderr.trim()}`)
  }
}

export async function prepareDevelopmentElectronIcon() {
  if (process.platform !== 'darwin') return

  const electronPackagePath = require.resolve('electron/package.json')
  const electronAppPath = join(dirname(electronPackagePath), 'dist', 'Electron.app')
  const resourcesPath = join(electronAppPath, 'Contents', 'Resources')
  const infoPlistPath = join(electronAppPath, 'Contents', 'Info.plist')
  const sourceIconPath = join(repositoryRoot, 'build', 'icon.icns')
  const iconBytes = await readFile(sourceIconPath)
  const iconFileName = createDevelopmentIconFileName(iconBytes)
  const destinationIconPath = join(resourcesPath, iconFileName)

  let destinationMatches = false
  try {
    const destinationBytes = await readFile(destinationIconPath)
    destinationMatches = destinationBytes.equals(iconBytes)
  } catch {
    destinationMatches = false
  }

  if (!destinationMatches) {
    await copyFile(sourceIconPath, destinationIconPath)
  }
  if (readBundleIconFile(infoPlistPath) !== iconFileName) {
    writeBundleIconFile(infoPlistPath, iconFileName)
  }

  // Bump the bundle timestamp so Launch Services does not reuse the stock Electron icon cache.
  const now = new Date()
  await utimes(electronAppPath, now, now)

  const installedIcon = await stat(destinationIconPath)
  if (installedIcon.size !== iconBytes.byteLength) {
    throw new Error('Electron development bundle icon verification failed.')
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  await prepareDevelopmentElectronIcon()
}
