/* eslint-disable @typescript-eslint/explicit-function-return-type -- electron-builder loads this JavaScript configuration directly. */

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { Arch } from 'electron-builder'
import { parse as parseYaml } from 'yaml'

const BASE_CONFIGURATION_URL = new URL('../electron-builder.yml', import.meta.url)
const PUBLISH_FIELDS = new Set(['provider', 'url', 'channel', 'useMultipleRangeRequest'])

export function validateUpdateUrl(value, { required = false } = {}) {
  if (value === undefined || value === '') {
    if (required) {
      throw new Error('CAPTAIN_WHO_UPDATE_URL is required for an update-enabled release')
    }
    return null
  }

  const invalid = () => {
    throw new Error(
      'CAPTAIN_WHO_UPDATE_URL must be an HTTPS directory URL without credentials, whitespace, query, or fragment'
    )
  }
  if (typeof value !== 'string' || /[\s\\?#]/.test(value)) invalid()
  let url
  try {
    url = new URL(value)
  } catch {
    invalid()
  }
  const authority = /^https:\/\/([^/]+)/i.exec(value)?.[1]
  if (
    url.protocol !== 'https:' ||
    !url.hostname ||
    !authority ||
    authority.includes('@') ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  ) {
    invalid()
  }
  if (!url.pathname.endsWith('/')) url.pathname += '/'
  return url.href
}

export function getMacUpdatePublishConfiguration(configuration) {
  if (configuration?.publish !== null) {
    throw new Error(
      'electron-builder publish must remain explicitly null to disable provider inference'
    )
  }
  for (const scope of ['win', 'linux', 'dmg', 'nsis', 'appImage', 'zip']) {
    const publish = configuration[scope]?.publish
    if (publish !== undefined && publish !== null) {
      throw new Error(`electron-builder ${scope}.publish must remain unset`)
    }
  }
  if (configuration.nsis?.differentialPackage !== false) {
    throw new Error('electron-builder nsis.differentialPackage must remain false')
  }

  const rawPublish = configuration.mac?.publish
  const publish = Array.isArray(rawPublish) && rawPublish.length === 1 ? rawPublish[0] : rawPublish
  if (publish === undefined || publish === null) {
    if (
      !Array.isArray(configuration.mac?.target) ||
      configuration.mac.target.length !== 1 ||
      configuration.mac.target[0] !== 'dmg'
    ) {
      throw new Error('electron-builder mac.target must remain DMG-only while auto-update is off')
    }
    if (configuration.dmg?.writeUpdateInfo !== false) {
      throw new Error(
        'electron-builder dmg.writeUpdateInfo must remain false while auto-update is off'
      )
    }
    return null
  }

  if (
    typeof publish !== 'object' ||
    Array.isArray(publish) ||
    publish.provider !== 'generic' ||
    publish.channel !== 'latest' ||
    publish.useMultipleRangeRequest !== false ||
    Object.keys(publish).some((key) => !PUBLISH_FIELDS.has(key))
  ) {
    throw new Error(
      'electron-builder mac.publish must be one credential-free generic latest provider'
    )
  }
  const url = validateUpdateUrl(publish.url, { required: true })
  if (publish.url !== url) {
    throw new Error('electron-builder mac.publish.url must be a normalized HTTPS directory URL')
  }
  const targets = configuration.mac.target
  if (
    !Array.isArray(targets) ||
    targets.length !== 2 ||
    !['dmg', 'zip'].every((name) =>
      targets.some(
        (target) =>
          target?.target === name &&
          Array.isArray(target.arch) &&
          target.arch.length === 1 &&
          target.arch[0] === 'arm64'
      )
    )
  ) {
    throw new Error('Update-enabled mac.target must contain exactly arm64 DMG and ZIP targets')
  }
  if (configuration.dmg?.writeUpdateInfo !== true || configuration.forceCodeSigning !== true) {
    throw new Error(
      'Update-enabled macOS releases require update metadata and forceCodeSigning=true'
    )
  }
  return publish
}

export function validateUpdateBuildTarget(context) {
  const publish = getMacUpdatePublishConfiguration(context?.packager?.config)
  if (!publish || !['darwin', 'mas'].includes(context.electronPlatformName)) return null
  if (context.electronPlatformName !== 'darwin' || context.arch !== Arch.arm64) {
    throw new Error('Desktop auto-update releases support macOS arm64 only')
  }
  const names = context.targets?.map((target) => target.name)
  if (names && (!names.includes('dmg') || !names.includes('zip'))) {
    throw new Error('Update-enabled macOS releases must build both DMG and ZIP targets')
  }
  if (context.packager.platformSpecificBuildOptions?.identity === null) {
    throw new Error('Update-enabled macOS releases cannot disable code signing')
  }
  return publish
}

export function createUpdateBuildConfiguration({
  environment = process.env,
  required = environment.CAPTAIN_WHO_REQUIRE_UPDATES === '1'
} = {}) {
  const url = validateUpdateUrl(environment.CAPTAIN_WHO_UPDATE_URL, { required })
  const configuration = parseYaml(readFileSync(BASE_CONFIGURATION_URL, 'utf8'))
  if (url) {
    configuration.forceCodeSigning = true
    configuration.mac.target = ['dmg', 'zip'].map((target) => ({ target, arch: ['arm64'] }))
    configuration.mac.publish = {
      provider: 'generic',
      url,
      channel: 'latest',
      useMultipleRangeRequest: false
    }
    configuration.dmg.writeUpdateInfo = true
  }
  configuration.beforePack = validateUpdateBuildTarget
  getMacUpdatePublishConfiguration(configuration)
  return configuration
}

export default function updateBuildConfiguration() {
  return createUpdateBuildConfiguration()
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    if (process.argv.slice(2).some((argument) => argument !== '--require-url')) {
      throw new Error('Usage: node scripts/update-config.mjs [--require-url]')
    }
    const configuration = createUpdateBuildConfiguration({
      required:
        process.argv.includes('--require-url') || process.env.CAPTAIN_WHO_REQUIRE_UPDATES === '1'
    })
    console.info(
      getMacUpdatePublishConfiguration(configuration)
        ? 'Update configuration valid: macOS arm64, generic HTTPS, DMG + ZIP; upload and notarization are separate.'
        : 'Update configuration valid: no update source; desktop auto-update is disabled.'
    )
  } catch (error) {
    console.error(error.message)
    process.exitCode = 1
  }
}
