/* eslint-disable @typescript-eslint/explicit-function-return-type -- electron-builder loads this JavaScript module directly. */

import { execFile as execFileCallback } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join, resolve } from 'node:path'
import { promisify } from 'node:util'
import { fileURLToPath } from 'node:url'

import { isMachOFile } from './frozen-macho-signing.mjs'
import { packagedMacApplicationPaths } from './verify-packaged-macos-signatures.mjs'

const execFile = promisify(execFileCallback)
const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)))

export function packagedMacNodePtyNativePaths(context) {
  const { app } = packagedMacApplicationPaths(context)
  const releaseDirectory = join(
    app,
    'Contents',
    'Resources',
    'app.asar.unpacked',
    'node_modules',
    'node-pty',
    'build',
    'Release'
  )
  return Object.freeze([join(releaseDirectory, 'pty.node'), join(releaseDirectory, 'spawn-helper')])
}

export async function sanitizePackagedMacNativeCode(
  context,
  { run = execFile, privatePathPrefixes = [repositoryRoot, homedir()] } = {}
) {
  if (typeof run !== 'function') {
    throw new Error('Packaged native-code sanitation requires a command runner')
  }
  if (
    !Array.isArray(privatePathPrefixes) ||
    privatePathPrefixes.length === 0 ||
    privatePathPrefixes.some((prefix) => typeof prefix !== 'string' || prefix.length === 0)
  ) {
    throw new Error('Packaged native-code sanitation requires private path prefixes')
  }
  const paths = packagedMacNodePtyNativePaths(context)
  for (const path of paths) {
    if (!(await isMachOFile(path, 'packaged node-pty native code'))) {
      throw new Error(`Packaged node-pty target is not Mach-O: ${path}`)
    }
    // Local compiler paths live in Mach-O debug/local-symbol data. Strip only those sections in
    // the packaged copy, before Developer ID signing, leaving the installed dependency untouched.
    await run('/usr/bin/strip', ['-S', '-x', path], {
      encoding: 'utf8',
      maxBuffer: 1024 * 1024
    })
    if (!(await isMachOFile(path, 'stripped packaged node-pty native code'))) {
      throw new Error(`Stripping damaged packaged node-pty Mach-O code: ${path}`)
    }
    const bytes = await readFile(path)
    for (const prefix of privatePathPrefixes) {
      if (bytes.includes(Buffer.from(prefix, 'utf8'))) {
        throw new Error('Packaged node-pty native code still contains a private build path')
      }
    }
  }
  return Object.freeze(paths)
}
