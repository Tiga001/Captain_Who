import { randomUUID } from 'node:crypto'
import { lstatSync, readFileSync } from 'node:fs'
import { mkdir, rename, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

export type AppearanceThemePreference = 'system' | 'light' | 'dark'

const APPEARANCE_FILE_NAME = 'appearance-v1.json'
const APPEARANCE_FILE_SCHEMA_VERSION = 1
const MAX_APPEARANCE_FILE_BYTES = 4_096
const DEFAULT_APPEARANCE_THEME: AppearanceThemePreference = 'system'

interface PersistedAppearanceTheme {
  schemaVersion: typeof APPEARANCE_FILE_SCHEMA_VERSION
  colorSchemePreference: AppearanceThemePreference
}

export interface AppearanceThemeMirror {
  getPreference(): AppearanceThemePreference
  setPreference(value: unknown): Promise<AppearanceThemePreference>
  beginShutdown(): Promise<void>
}

export function isAppearanceThemePreference(value: unknown): value is AppearanceThemePreference {
  return value === 'system' || value === 'light' || value === 'dark'
}

export function resolveAppearanceColorScheme(
  preference: AppearanceThemePreference,
  systemIsDark: boolean
): 'light' | 'dark' {
  if (preference === 'dark' || preference === 'light') return preference
  return systemIsDark ? 'dark' : 'light'
}

/**
 * A narrow Main-owned mirror of the Renderer appearance preference.
 *
 * The Renderer remains the product-setting authority. This file only lets Main set
 * nativeTheme before the first window exists on the next cold start.
 */
export class AppearanceThemeStore implements AppearanceThemeMirror {
  private readonly filePath: string
  private acceptingWrites = true
  private preference: AppearanceThemePreference
  private writes: Promise<void> = Promise.resolve()

  constructor(private readonly userDataDirectory: string) {
    this.filePath = join(userDataDirectory, APPEARANCE_FILE_NAME)
    this.preference = readPersistedAppearance(this.filePath)
  }

  getPreference(): AppearanceThemePreference {
    return this.preference
  }

  setPreference(value: unknown): Promise<AppearanceThemePreference> {
    if (!isAppearanceThemePreference(value)) {
      return Promise.reject(new Error('Invalid native theme source'))
    }
    if (!this.acceptingWrites) {
      return Promise.reject(new Error('Appearance theme store is shutting down'))
    }
    const preference = value
    this.preference = preference

    const write = this.writes.then(() =>
      persistAppearance(this.userDataDirectory, this.filePath, preference)
    )
    this.writes = write.catch(() => undefined)
    return write.then(() => preference)
  }

  beginShutdown(): Promise<void> {
    this.acceptingWrites = false
    return this.writes
  }
}

export function createVolatileAppearanceThemeMirror(): AppearanceThemeMirror {
  let preference: AppearanceThemePreference = DEFAULT_APPEARANCE_THEME
  let stopped = false
  return {
    getPreference: () => preference,
    setPreference: (value) => {
      if (!isAppearanceThemePreference(value)) {
        return Promise.reject(new Error('Invalid native theme source'))
      }
      if (stopped) return Promise.reject(new Error('Appearance theme store is shutting down'))
      preference = value
      return Promise.resolve(value)
    },
    beginShutdown: () => {
      stopped = true
      return Promise.resolve()
    }
  }
}

function readPersistedAppearance(filePath: string): AppearanceThemePreference {
  try {
    const metadata = lstatSync(filePath)
    if (
      !metadata.isFile() ||
      metadata.isSymbolicLink() ||
      metadata.size > MAX_APPEARANCE_FILE_BYTES
    ) {
      return DEFAULT_APPEARANCE_THEME
    }
    const value: unknown = JSON.parse(readFileSync(filePath, 'utf8'))
    if (
      !isExactAppearanceRecord(value) ||
      !isAppearanceThemePreference(value.colorSchemePreference)
    ) {
      return DEFAULT_APPEARANCE_THEME
    }
    return value.colorSchemePreference
  } catch {
    return DEFAULT_APPEARANCE_THEME
  }
}

function isExactAppearanceRecord(value: unknown): value is Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false
  const record = value as Record<string, unknown>
  const keys = Object.keys(record).sort()
  return (
    keys.length === 2 &&
    keys[0] === 'colorSchemePreference' &&
    keys[1] === 'schemaVersion' &&
    record.schemaVersion === APPEARANCE_FILE_SCHEMA_VERSION
  )
}

async function persistAppearance(
  userDataDirectory: string,
  filePath: string,
  colorSchemePreference: AppearanceThemePreference
): Promise<void> {
  await mkdir(userDataDirectory, { recursive: true, mode: 0o700 })
  const temporaryPath = join(
    userDataDirectory,
    `.${APPEARANCE_FILE_NAME}.${randomUUID()}.temporary`
  )
  const value: PersistedAppearanceTheme = {
    schemaVersion: APPEARANCE_FILE_SCHEMA_VERSION,
    colorSchemePreference
  }
  try {
    await writeFile(temporaryPath, `${JSON.stringify(value)}\n`, {
      encoding: 'utf8',
      flag: 'wx',
      mode: 0o600
    })
    await rename(temporaryPath, filePath)
  } catch (error) {
    await rm(temporaryPath, { force: true }).catch(() => undefined)
    throw error
  }
}
