/* eslint-disable @typescript-eslint/explicit-function-return-type -- Test fixtures intentionally use compact callbacks. */

import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'

import {
  packagedMacNodePtyNativePaths,
  sanitizePackagedMacNativeCode
} from './sanitize-packaged-mac-native-code.mjs'

function context(appOutDir) {
  return {
    electronPlatformName: 'darwin',
    appOutDir,
    packager: { appInfo: { productFilename: 'MyCopilot' } }
  }
}

test('packaged node-pty sanitation strips both exact Mach-O paths and removes the private prefix', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-native-sanitize-'))
  const privatePrefix = '/private/build-user/project'
  const paths = packagedMacNodePtyNativePaths(context(directory))
  const machOHeader = Buffer.from([0xcf, 0xfa, 0xed, 0xfe])
  for (const path of paths) {
    await mkdir(dirname(path), { recursive: true })
    await writeFile(path, Buffer.concat([machOHeader, Buffer.from(privatePrefix)]))
  }
  const calls = []
  const sanitized = await sanitizePackagedMacNativeCode(context(directory), {
    privatePathPrefixes: [privatePrefix],
    async run(command, argumentsList) {
      calls.push({ command, argumentsList })
      await writeFile(argumentsList.at(-1), machOHeader)
      return { stdout: '', stderr: '' }
    }
  })

  assert.deepEqual(sanitized, paths)
  assert.equal(calls.length, 2)
  assert.equal(
    calls.every(({ command }) => command === '/usr/bin/strip'),
    true
  )
  assert.equal(
    calls.every(({ argumentsList }) =>
      ['-S', '-x'].every((argument) => argumentsList.includes(argument))
    ),
    true
  )
  for (const path of paths) {
    assert.equal((await readFile(path)).includes(Buffer.from(privatePrefix)), false)
  }
})

test('packaged node-pty sanitation fails closed if stripping leaves a private build path', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-native-sanitize-fail-'))
  const privatePrefix = '/private/build-user/project'
  for (const path of packagedMacNodePtyNativePaths(context(directory))) {
    await mkdir(dirname(path), { recursive: true })
    await writeFile(
      path,
      Buffer.concat([Buffer.from([0xcf, 0xfa, 0xed, 0xfe]), Buffer.from(privatePrefix)])
    )
  }
  await assert.rejects(
    sanitizePackagedMacNativeCode(context(directory), {
      privatePathPrefixes: [privatePrefix],
      async run() {
        return { stdout: '', stderr: '' }
      }
    }),
    /still contains a private build path/
  )
})
