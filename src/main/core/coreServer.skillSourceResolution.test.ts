import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import type {
  SkillSourceResolutionErrorData,
  SkillsCancelSourceResolutionInput,
  SkillsCancelSourceResolutionOutput,
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
  cancel: {
    cases: Array<{
      request: SkillsCancelSourceResolutionInput
      response: SkillsCancelSourceResolutionOutput
    }>
  }
  errors: SkillSourceResolutionErrorData[]
}

const golden = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/protocol/fixtures/skill-source-resolution-v2.json'),
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

  it('routes idempotent cancellation and validates the matching resolution identity', async () => {
    const testCase = golden.cancel.cases[0]
    if (!testCase) throw new Error('The source resolution fixture must contain cancellation')
    rpcRequest.mockResolvedValueOnce(testCase.response)

    await expect(new CoreServer().cancelSkillSourceResolution(testCase.request)).resolves.toEqual(
      testCase.response
    )
    expect(rpcRequest).toHaveBeenCalledWith('skills.cancelSourceResolution', testCase.request)

    rpcRequest.mockResolvedValueOnce({
      ...testCase.response,
      resolutionId: '99999999-9999-4999-8999-999999999999'
    })
    await expect(new CoreServer().cancelSkillSourceResolution(testCase.request)).rejects.toThrow(
      'resolutionId must match request.resolutionId'
    )
  })

  it('rejects an invalid client resolution id before sending an RPC request', () => {
    const testCase = golden.cases[0]
    if (!testCase) throw new Error('The source resolution fixture must contain a case')

    expect(() =>
      new CoreServer().resolveSkillInstallationSource({
        ...testCase.request,
        resolutionId: '00000000-0000-0000-0000-000000000000'
      })
    ).toThrow('canonical non-nil lower-case UUID')
    expect(rpcRequest).not.toHaveBeenCalled()
  })

  it('rejects malformed responses before they enter typed application code', async () => {
    const testCase = golden.cases[0]
    if (!testCase) throw new Error('The source resolution fixture must contain a case')
    rpcRequest.mockResolvedValueOnce({ ...testCase.response, resolvedCommit: 'mutable-main' })

    await expect(new CoreServer().resolveSkillInstallationSource(testCase.request)).rejects.toThrow(
      '40-character hexadecimal SHA'
    )
  })

  it('rejects a response belonging to a different resolution request', async () => {
    const testCase = golden.cases[0]
    if (!testCase) throw new Error('The source resolution fixture must contain a case')
    rpcRequest.mockResolvedValueOnce({
      ...testCase.response,
      resolutionId: '99999999-9999-4999-8999-999999999999',
      candidates: testCase.response.candidates.map((candidate) => ({
        ...candidate,
        acquisition: {
          ...candidate.acquisition,
          resolutionId: '99999999-9999-4999-8999-999999999999'
        }
      }))
    })

    await expect(new CoreServer().resolveSkillInstallationSource(testCase.request)).rejects.toThrow(
      'resolutionId must match request.resolutionId'
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

  it.each(['resolutionIdConflict', 'capacityExceeded'] as const)(
    'preserves actionable %s registry recovery data',
    async (code) => {
      const testCase = golden.cases[0]
      const errorData = golden.errors.find((error) => error.code === code)
      if (!testCase || !errorData) {
        throw new Error(`The source resolution fixture must cover ${code}`)
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
        code: -32013,
        data: errorData
      })
    }
  )

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
