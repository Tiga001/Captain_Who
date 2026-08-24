import { randomUUID } from 'node:crypto'
import { lstatSync, readFileSync } from 'node:fs'
import { mkdir, rename, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import {
  DEFAULT_APP_LANGUAGE,
  isAppLanguage,
  type AppLanguage
} from '../../shared/i18n/languageRegistry'

const LOCALE_FILE_NAME = 'notification-locale-v1.json'
const LOCALE_FILE_SCHEMA_VERSION = 1
const MAX_LOCALE_FILE_BYTES = 4_096

interface PersistedNotificationLocale {
  schemaVersion: typeof LOCALE_FILE_SCHEMA_VERSION
  language: AppLanguage
}

export interface NotificationLocaleMirror {
  getLocale(): AppLanguage
  setLocale(value: unknown): Promise<AppLanguage>
  beginShutdown(): Promise<void>
}

/**
 * A narrow Main-owned mirror of the Renderer application's canonical language setting.
 *
 * The Renderer remains the product-setting authority. This file only lets Main format native
 * notifications before a Renderer exists on the next cold start. Values are parsed with the
 * shared application language registry, so Main cannot grow a second locale catalog.
 */
export class NotificationLocaleStore implements NotificationLocaleMirror {
  private readonly filePath: string
  private acceptingWrites = true
  private language: AppLanguage
  private writes: Promise<void> = Promise.resolve()

  constructor(private readonly userDataDirectory: string) {
    this.filePath = join(userDataDirectory, LOCALE_FILE_NAME)
    this.language = readPersistedLocale(this.filePath)
  }

  getLocale(): AppLanguage {
    return this.language
  }

  setLocale(value: unknown): Promise<AppLanguage> {
    if (!isAppLanguage(value)) return Promise.reject(new Error('Invalid application language'))
    if (!this.acceptingWrites) {
      return Promise.reject(new Error('Notification locale store is shutting down'))
    }
    const language = value
    // Session behavior must follow the user's visible setting immediately. Persistence is only a
    // cold-start mirror; a slow or failed older write must never roll Main back in memory.
    this.language = language

    const write = this.writes.then(() =>
      persistLocale(this.userDataDirectory, this.filePath, language)
    )
    // Keep the serialization chain alive after an individual filesystem error. The caller still
    // receives that exact rejection and Main does not claim the failed value in memory.
    this.writes = write.catch(() => undefined)
    return write.then(() => language)
  }

  beginShutdown(): Promise<void> {
    this.acceptingWrites = false
    return this.writes
  }
}

export function createVolatileNotificationLocaleMirror(): NotificationLocaleMirror {
  let language = DEFAULT_APP_LANGUAGE
  let stopped = false
  return {
    getLocale: () => language,
    setLocale: (value) => {
      if (!isAppLanguage(value)) return Promise.reject(new Error('Invalid application language'))
      if (stopped) return Promise.reject(new Error('Notification locale store is shutting down'))
      language = value
      return Promise.resolve(value)
    },
    beginShutdown: () => {
      stopped = true
      return Promise.resolve()
    }
  }
}

function readPersistedLocale(filePath: string): AppLanguage {
  try {
    const metadata = lstatSync(filePath)
    if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size > MAX_LOCALE_FILE_BYTES) {
      return DEFAULT_APP_LANGUAGE
    }
    const value: unknown = JSON.parse(readFileSync(filePath, 'utf8'))
    if (!isExactLocaleRecord(value) || !isAppLanguage(value.language)) {
      return DEFAULT_APP_LANGUAGE
    }
    return value.language
  } catch {
    return DEFAULT_APP_LANGUAGE
  }
}

function isExactLocaleRecord(value: unknown): value is Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false
  const record = value as Record<string, unknown>
  const keys = Object.keys(record).sort()
  return (
    keys.length === 2 &&
    keys[0] === 'language' &&
    keys[1] === 'schemaVersion' &&
    record.schemaVersion === LOCALE_FILE_SCHEMA_VERSION
  )
}

async function persistLocale(
  userDataDirectory: string,
  filePath: string,
  language: AppLanguage
): Promise<void> {
  await mkdir(userDataDirectory, { recursive: true, mode: 0o700 })
  const temporaryPath = join(userDataDirectory, `.${LOCALE_FILE_NAME}.${randomUUID()}.temporary`)
  const value: PersistedNotificationLocale = {
    schemaVersion: LOCALE_FILE_SCHEMA_VERSION,
    language
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
