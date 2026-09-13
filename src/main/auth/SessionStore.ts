import { readFileSync, writeFileSync, renameSync, unlinkSync, mkdirSync } from 'node:fs'
import { join } from 'node:path'

export interface SessionTokens {
  access_token: string
  refresh_token: string
}

export interface SessionStorage {
  read(): SessionTokens | null
  write(tokens: SessionTokens): boolean
  clear(): void
}

interface Encryption {
  isEncryptionAvailable(): boolean
  encryptString(value: string): Buffer
  decryptString(value: Buffer): string
}

export class SessionStore implements SessionStorage {
  private readonly file: string

  constructor(
    directory: string,
    private readonly encryption: Encryption,
    private readonly scope: string
  ) {
    mkdirSync(directory, { recursive: true, mode: 0o700 })
    this.file = join(directory, 'account-session.enc')
  }

  read(): SessionTokens | null {
    let encrypted: Buffer
    try {
      encrypted = readFileSync(this.file)
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return null
      throw new Error('Session storage unavailable')
    }
    const value = JSON.parse(this.encryption.decryptString(encrypted))
    if (value?.scope !== this.scope) {
      // Unscoped or foreign-environment sessions must never reach the authentication driver.
      this.clear()
      return null
    }
    if (typeof value.access_token !== 'string' || typeof value.refresh_token !== 'string') {
      throw new Error('Invalid session storage')
    }
    return { access_token: value.access_token, refresh_token: value.refresh_token }
  }

  write(tokens: SessionTokens): boolean {
    if (!this.encryption.isEncryptionAvailable()) return false
    const encrypted = this.encryption.encryptString(
      JSON.stringify({
        scope: this.scope,
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token
      })
    )
    writeFileSync(`${this.file}.tmp`, encrypted, { mode: 0o600 })
    renameSync(`${this.file}.tmp`, this.file)
    return true
  }

  clear(): void {
    for (const file of [this.file, `${this.file}.tmp`]) {
      try {
        unlinkSync(file)
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error
      }
    }
  }
}
