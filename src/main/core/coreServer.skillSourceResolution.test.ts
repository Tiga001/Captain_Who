import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import type {
  SkillSourceResolutionErrorData,
  SkillsResolveInstallationSourceInput,
  SkillsResolveInstallationSourceOutput
} from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const rpcRequest = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = rpcRequest
  }
}))

import { CoreServer } from './coreServer'

interface SkillSourceResolutionGolden {
  cases: Array<{
    request: SkillsResolveInstallationSourceInput
    response: SkillsResolveInstallationSourceOutput
  }>
  errors: SkillSourceResolutionErrorData[]
}

const golden = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/protocol/fixtures/skill-source-resolution-v1.json'),
    'utf8'
  )
) as SkillSourceResolutionGolden

describe('CoreServer Skill source resolution client', () => {
  beforeEach(() => {
    rpcRequest.mockReset()
  })

  it('routes resolution through its stable RPC method and validates the response', async () => {
    const testCase = golden.cases[0]
    if (!testCase) throw new Error('The source resolution fixture must contain a case')
    rpcRequest.mockResolvedValueOnce(testCase.response)

    await expect(
      new CoreServer().resolveSkillInstallationSource(testCase.request)
    ).resolves.toEqual(testCase.response)
    expect(rpcRequest).toHaveBeenCalledWith('skills.resolveInstallationSource', testCase.request)
  })

  it('rejects malformed responses before they enter typed application code', async () => {
    const testCase = golden.cases[0]
    if (!testCase) throw new Error('The source resolution fixture must contain a case')
    rpcRequest.mockResolvedValueOnce({ ...testCase.response, resolvedCommit: 'mutable-main' })

    await expect(new CoreServer().resolveSkillInstallationSource(testCase.request)).rejects.toThrow(
      '40-character hexadecimal SHA'
    )
  })

  it('validates and preserves structured resolution errors for the IPC envelope', async () => {
    const testCase = golden.cases[0]
    const errorData = golden.errors.find((error) => error.code === 'rateLimited')
    if (!testCase || !errorData) {
      throw new Error('The source resolution fixture must cover a rate-limited request')
    }
    rpcRequest.mockRejectedValueOnce(
      Object.assign(new Error(errorData.message), {
        code: -32013,
        data: errorData
      })
    )

    await expect(
      new CoreServer().resolveSkillInstallationSource(testCase.request)
    ).rejects.toMatchObject({
      message: errorData.message,
      code: -32013,
      data: errorData
    })
  })

  it('rejects malformed structured resolution errors at the JSON-RPC boundary', async () => {
    const testCase = golden.cases[0]
    if (!testCase) throw new Error('The source resolution fixture must contain a case')
    rpcRequest.mockRejectedValueOnce(
      Object.assign(new Error('Malformed resolution error'), {
        code: -32013,
        data: {
          type: 'skillSourceResolution',
          phase: 'resolve',
          code: 'rateLimited'
        }
      })
    )

    await expect(new CoreServer().resolveSkillInstallationSource(testCase.request)).rejects.toThrow(
      'Skill source resolution error data'
    )
  })
})
