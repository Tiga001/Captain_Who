import { constants } from 'node:fs'
import { access, open, stat } from 'node:fs/promises'
import { resolve } from 'node:path'

const binaryName = process.platform === 'win32' ? 'core-server.exe' : 'core-server'
const binaryPath = resolve('target', 'release', binaryName)
const expectedMagic = {
  darwin: new Set(['cffaedfe', 'feedfacf', 'cafebabe', 'cafebabf']),
  linux: new Set(['7f454c46']),
  win32: new Set(['4d5a'])
}[process.platform]

if (!expectedMagic) {
  throw new Error(`Unsupported packaging host: ${process.platform}`)
}

const metadata = await stat(binaryPath).catch(() => null)
if (!metadata?.isFile() || metadata.size === 0) {
  throw new Error(`Missing release core binary: ${binaryPath}`)
}

if (process.platform !== 'win32') {
  await access(binaryPath, constants.X_OK)
}

const handle = await open(binaryPath, 'r')
const signature = Buffer.alloc(4)
try {
  await handle.read(signature, 0, signature.length, 0)
} finally {
  await handle.close()
}

const signatureHex = signature.toString('hex')
const hasExpectedMagic = [...expectedMagic].some((magic) => signatureHex.startsWith(magic))
if (!hasExpectedMagic) {
  throw new Error(`Core binary has the wrong format for ${process.platform}: ${binaryPath}`)
}

console.log(`Verified ${binaryName} (${metadata.size} bytes) for ${process.platform}.`)
