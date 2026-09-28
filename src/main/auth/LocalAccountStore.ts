import { randomUUID } from 'node:crypto'
import { mkdirSync, readFileSync, renameSync, unlinkSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import type { AccountProfile } from '@mycopilot/host-api'

export const LOCAL_ACCOUNT_USERNAME = 'captainwho'
const LOCAL_ACCOUNT_PASSWORD = 'captainwho'

export interface LocalAccountSession {
  avatarSeed: string
}

export interface LocalAccountStorage {
  read(): LocalAccountSession | null
  activate(): LocalAccountSession
  clear(): void
}

interface SavedLocalAccount extends LocalAccountSession {
  version: 1
  username: typeof LOCAL_ACCOUNT_USERNAME
  signedIn: boolean
}

export function acceptsLocalPassword(password: unknown): boolean {
  return password === LOCAL_ACCOUNT_PASSWORD
}

export function localAccountProfile(session: LocalAccountSession): AccountProfile {
  return {
    userId: 'local:captainwho',
    displayName: '大副',
    email: '',
    avatarDataUrl: null,
    occupation: '',
    organization: '',
    localAccount: { username: LOCAL_ACCOUNT_USERNAME, avatarSeed: session.avatarSeed }
  }
}

/**
 * The explicitly supported built-in offline account has no cloud credentials or expiry.
 * This file remembers only its local sign-in choice and bundled-avatar seed, not a password
 * or cloud grant. It deliberately does not depend on Keychain availability or cloud scope.
 */
export class LocalAccountStore implements LocalAccountStorage {
  private readonly file: string

  constructor(directory: string) {
    mkdirSync(directory, { recursive: true, mode: 0o700 })
    this.file = join(directory, 'local-account.json')
  }

  private readRecord(): SavedLocalAccount | null {
    let text: string
    try {
      text = readFileSync(this.file, 'utf8')
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return null
      throw error
    }
    try {
      const value = JSON.parse(text)
      if (
        value?.version !== 1 ||
        value.username !== LOCAL_ACCOUNT_USERNAME ||
        typeof value.signedIn !== 'boolean' ||
        typeof value.avatarSeed !== 'string' ||
        !/^[0-9a-f-]{36}$/i.test(value.avatarSeed)
      )
        return null
      return value
    } catch {
      return null
    }
  }

  private write(record: SavedLocalAccount): void {
    writeFileSync(`${this.file}.tmp`, JSON.stringify(record), { mode: 0o600 })
    renameSync(`${this.file}.tmp`, this.file)
  }

  read(): LocalAccountSession | null {
    const record = this.readRecord()
    return record?.signedIn ? { avatarSeed: record.avatarSeed } : null
  }

  activate(): LocalAccountSession {
    const avatarSeed = this.readRecord()?.avatarSeed ?? randomUUID()
    this.write({ version: 1, username: LOCAL_ACCOUNT_USERNAME, signedIn: true, avatarSeed })
    return { avatarSeed }
  }

  clear(): void {
    const record = this.readRecord()
    if (record?.signedIn) this.write({ ...record, signedIn: false })
    for (const file of record ? [`${this.file}.tmp`] : [this.file, `${this.file}.tmp`]) {
      try {
        unlinkSync(file)
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error
      }
    }
  }
}
