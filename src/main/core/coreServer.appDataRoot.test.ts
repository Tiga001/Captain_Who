import { resolve } from 'node:path'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const constructClient = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    constructor(options?: unknown) {
      constructClient(options)
    }
  }
}))

import { CoreServer } from './coreServer'

describe('CoreServer application data root', () => {
  beforeEach(() => constructClient.mockReset())

  it('passes the Host-owned root to its JSON-RPC client unchanged', () => {
    const appDataRoot = resolve('fixtures', 'MyCopilot User Data')

    new CoreServer({ appDataRoot })

    expect(constructClient).toHaveBeenCalledOnce()
    expect(constructClient).toHaveBeenCalledWith({ appDataRoot })
  })

  it('preserves source-compatible construction for isolated mocked clients', () => {
    new CoreServer()

    expect(constructClient).toHaveBeenCalledWith(undefined)
  })
})
