export interface GitReviewPayloadCacheBudgetOptions {
  maxCharacters: number
  maxFiles: number
}

/** Tracks text payloads by recent demand and identifies cold entries that must leave React state. */
export class GitReviewPayloadCacheBudget {
  readonly #entries = new Map<string, number>()
  readonly #maxCharacters: number
  readonly #maxFiles: number
  #totalCharacters = 0

  constructor(options: GitReviewPayloadCacheBudgetOptions) {
    if (!Number.isSafeInteger(options.maxCharacters) || options.maxCharacters < 1) {
      throw new Error('Git review cache character budget must be a positive integer.')
    }
    if (!Number.isSafeInteger(options.maxFiles) || options.maxFiles < 1) {
      throw new Error('Git review cache file budget must be a positive integer.')
    }
    this.#maxCharacters = options.maxCharacters
    this.#maxFiles = options.maxFiles
  }

  clear(): void {
    this.#entries.clear()
    this.#totalCharacters = 0
  }

  record(
    fileId: string,
    characterCount: number,
    protectedFileIds: ReadonlySet<string> = EMPTY_FILE_IDS
  ): readonly string[] {
    if (!Number.isSafeInteger(characterCount) || characterCount < 0) {
      throw new Error('Git review cache entry size must be a non-negative integer.')
    }
    this.remove(fileId)
    this.#entries.set(fileId, characterCount)
    this.#totalCharacters += characterCount
    return this.trim(protectedFileIds)
  }

  /** Trims cold entries only. A hot working set may temporarily exceed the configured budget. */
  trim(protectedFileIds: ReadonlySet<string> = EMPTY_FILE_IDS): readonly string[] {
    const evicted: string[] = []
    while (this.#entries.size > this.#maxFiles || this.#totalCharacters > this.#maxCharacters) {
      let oldestFileId: string | undefined
      for (const candidate of this.#entries.keys()) {
        if (!protectedFileIds.has(candidate)) {
          oldestFileId = candidate
          break
        }
      }
      if (!oldestFileId) break
      this.remove(oldestFileId)
      evicted.push(oldestFileId)
    }
    return evicted
  }

  remove(fileId: string): void {
    const characterCount = this.#entries.get(fileId)
    if (characterCount === undefined) return
    this.#entries.delete(fileId)
    this.#totalCharacters = Math.max(0, this.#totalCharacters - characterCount)
  }

  touch(fileId: string): void {
    const characterCount = this.#entries.get(fileId)
    if (characterCount === undefined) return
    this.#entries.delete(fileId)
    this.#entries.set(fileId, characterCount)
  }
}

const EMPTY_FILE_IDS: ReadonlySet<string> = new Set()
