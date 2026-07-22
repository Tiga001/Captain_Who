import type { OfficeEngineStatus, OfficeHelpVerb } from '@mycopilot/protocol'
import { OFFICE_GET_STATUS_METHOD } from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const rpcRequest = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = rpcRequest
  }
}))

import { CoreServer } from './coreServer'

const status = {
  schemaVersion: 1,
  providerId: 'officecli',
  availability: 'available',
  source: 'configured',
  version: 'OfficeCLI 1.0.139',
  engineRevision: 'office-engine-sha256-v1:test',
  capabilities: {
    providerId: 'officecli',
    documentKinds: ['document', 'spreadsheet', 'presentation'],
    operations: ['help', 'create', 'view', 'get', 'query', 'validate'],
    supportsRendering: true,
    supportsValidation: true,
    supportsStructuredOutput: true
  }
} satisfies OfficeEngineStatus

const officeHelpTopics = [
  'status',
  'help',
  'create',
  'view',
  'get',
  'query',
  'validate',
  'set',
  'add',
  'remove',
  'move',
  'swap'
] as const satisfies readonly OfficeHelpVerb[]

describe('CoreServer Office status client', () => {
  beforeEach(() => rpcRequest.mockReset())

  it('uses the stable no-parameter method and validates its result', async () => {
    rpcRequest.mockResolvedValue(status)

    await expect(new CoreServer().getOfficeStatus()).resolves.toEqual(status)
    expect(rpcRequest).toHaveBeenCalledWith(OFFICE_GET_STATUS_METHOD)
  })

  it('keeps Host-managed and provider element help topics in the shared protocol', () => {
    expect(officeHelpTopics).toEqual([
      'status',
      'help',
      'create',
      'view',
      'get',
      'query',
      'validate',
      'set',
      'add',
      'remove',
      'move',
      'swap'
    ])
  })

  it.each([
    { ...status, schemaVersion: 2 },
    { ...status, availability: 'degraded' },
    { ...status, executablePath: '/must/not/cross/the/protocol' },
    { ...status, capabilities: { ...status.capabilities, providerId: 'different-provider' } },
    { ...status, capabilities: { ...status.capabilities, operations: ['raw'] } }
  ])('rejects malformed or expanded wire values', async (invalid) => {
    rpcRequest.mockResolvedValue(invalid)

    await expect(new CoreServer().getOfficeStatus()).rejects.toThrow('Invalid Office engine status')
  })
})
