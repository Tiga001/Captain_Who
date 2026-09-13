import { mkdirSync, readFileSync, writeFileSync, renameSync } from 'node:fs'
import { join } from 'node:path'
import { LICENSE_CACHE_MS, type VerifiedLicense } from './LicenseApiClient'

export interface CachedLicense {
  userId: string
  receivedAt: number
  license: VerifiedLicense
}
export interface LicenseStorage {
  read(userId: string): CachedLicense | null
  write(entry: CachedLicense): boolean
}
interface Encryption {
  isEncryptionAvailable(): boolean
  encryptString(value: string): Buffer
  decryptString(value: Buffer): string
}

/** No tokens or hardware identifiers. A short-lived cache, never an entitlement authority. */
export class LicenseCacheStore implements LicenseStorage {
  private readonly file: string
  constructor(
    directory: string,
    private readonly encryption: Encryption,
    private readonly scope: string
  ) {
    mkdirSync(directory, { recursive: true, mode: 0o700 })
    this.file = join(directory, 'account-license.enc')
  }
  private entries(): CachedLicense[] {
    try {
      const value = JSON.parse(this.encryption.decryptString(readFileSync(this.file)))
      return value?.scope === this.scope && Array.isArray(value.entries) ? value.entries : []
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return []
      throw new Error('License cache unavailable')
    }
  }
  read(userId: string): CachedLicense | null {
    return this.entries().find((entry) => entry?.userId === userId) ?? null
  }
  write(entry: CachedLicense): boolean {
    if (!this.encryption.isEncryptionAvailable()) return false
    let previous: CachedLicense[]
    try {
      previous = this.entries()
    } catch {
      previous = []
    }
    const entries = previous
      .filter(
        (item) =>
          item?.userId !== entry.userId &&
          Number.isFinite(item?.receivedAt) &&
          item.receivedAt <= entry.receivedAt &&
          entry.receivedAt - item.receivedAt < LICENSE_CACHE_MS
      )
      .slice(-31)
    const bytes = this.encryption.encryptString(
      JSON.stringify({ scope: this.scope, entries: [...entries, entry] })
    )
    writeFileSync(`${this.file}.tmp`, bytes, { mode: 0o600 })
    renameSync(`${this.file}.tmp`, this.file)
    return true
  }
}
