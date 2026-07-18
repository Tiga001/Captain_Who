import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import type {
  SkillInstallationCommitOutput,
  SkillInstallationPreview,
  SkillManagementErrorData,
  SkillPreparationCancellationOutput,
  SkillsCancelPreparationInput,
  SkillsChangedNotification,
  SkillsCommitInstallationInput,
  SkillsInspectInstallationInput,
  SkillsListManagementInput,
  SkillsListManagementOutput,
  SkillsSetEnabledInput,
  SkillsSetEnabledOutput
} from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const rpcRequest = vi.hoisted(() => vi.fn())
const rpcOnNotification = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = rpcRequest
    readonly onNotification = rpcOnNotification
  }
}))

import { CoreServer } from './coreServer'

interface SkillWorkflowGolden {
  inspectCases: Array<{
    request: SkillsInspectInstallationInput
    preview: SkillInstallationPreview
  }>
  commit: {
    request: SkillsCommitInstallationInput
    response: SkillInstallationCommitOutput
  }
  cancel: {
    request: SkillsCancelPreparationInput
    response: SkillPreparationCancellationOutput
  }
  management: {
    listRequest: SkillsListManagementInput
    listResponse: SkillsListManagementOutput
    setEnabledRequest: SkillsSetEnabledInput
    setEnabledResponse: SkillsSetEnabledOutput
    changed: SkillsChangedNotification
  }
  inspectionError: {
    type: 'skillInspection'
    phase: 'inspect'
    code: 'rateLimited'
    recovery: 'retryLater'
    message: string
    preparationId: string
    diagnosticCode: string
    retryAfterMs: number
  }
  managementErrors: SkillManagementErrorData[]
}

const golden = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/protocol/fixtures/skill-installation-workflow-v1.json'),
    'utf8'
  )
) as SkillWorkflowGolden

describe('CoreServer Skill installation workflow client', () => {
  beforeEach(() => {
    rpcRequest.mockReset()
    rpcOnNotification.mockReset()
  })

  it('routes each workflow request through its stable RPC method and runtime parser', async () => {
    const inspect = golden.inspectCases[0]
    if (!inspect) throw new Error('The workflow fixture must contain an inspection case')
    rpcRequest
      .mockResolvedValueOnce(inspect.preview)
      .mockResolvedValueOnce(golden.commit.response)
      .mockResolvedValueOnce(golden.cancel.response)
      .mockResolvedValueOnce(golden.management.listResponse)
      .mockResolvedValueOnce(golden.management.setEnabledResponse)
    const server = new CoreServer()

    await expect(server.inspectSkillInstallation(inspect.request)).resolves.toEqual(inspect.preview)
    await expect(server.commitSkillInstallation(golden.commit.request)).resolves.toEqual(
      golden.commit.response
    )
    await expect(server.cancelSkillPreparation(golden.cancel.request)).resolves.toEqual(
      golden.cancel.response
    )
    await expect(server.listSkillManagement(golden.management.listRequest)).resolves.toEqual(
      golden.management.listResponse
    )
    await expect(server.setSkillEnabled(golden.management.setEnabledRequest)).resolves.toEqual(
      golden.management.setEnabledResponse
    )

    expect(rpcRequest.mock.calls).toEqual([
      ['skills.inspectInstallation', inspect.request],
      ['skills.commitInstallation', golden.commit.request],
      ['skills.cancelPreparation', golden.cancel.request],
      ['skills.listManagement', golden.management.listRequest],
      ['skills.setEnabled', golden.management.setEnabledRequest]
    ])
  })

  it('rejects malformed values before they enter typed application code', async () => {
    const inspect = golden.inspectCases[0]
    if (!inspect) throw new Error('The workflow fixture must contain an inspection case')
    const invalidResponses: Array<{
      response: unknown
      invoke(server: CoreServer): Promise<unknown>
    }> = [
      {
        response: { ...inspect.preview, schemaVersion: 99 },
        invoke: (server) => server.inspectSkillInstallation(inspect.request)
      },
      {
        response: { ...golden.commit.response, operation: 'install', outcome: 'updated' },
        invoke: (server) => server.commitSkillInstallation(golden.commit.request)
      },
      {
        response: { ...golden.cancel.response, outcome: 'forgotten' },
        invoke: (server) => server.cancelSkillPreparation(golden.cancel.request)
      },
      {
        response: { ...golden.management.listResponse, skills: null },
        invoke: (server) => server.listSkillManagement(golden.management.listRequest)
      },
      {
        response: { ...golden.management.setEnabledResponse, enabled: 'false' },
        invoke: (server) => server.setSkillEnabled(golden.management.setEnabledRequest)
      }
    ]
    const server = new CoreServer()

    for (const testCase of invalidResponses) {
      rpcRequest.mockResolvedValueOnce(testCase.response)
      await expect(testCase.invoke(server)).rejects.toThrow()
    }
  })

  it('preserves structured RPC errors for the IPC invocation envelope', async () => {
    const structuredError = Object.assign(new Error('GitHub rate limit reached'), {
      code: -32011,
      data: golden.inspectionError
    })
    const inspect = golden.inspectCases[0]
    if (!inspect) throw new Error('The workflow fixture must contain an inspection case')
    rpcRequest.mockRejectedValueOnce(structuredError)

    await expect(new CoreServer().inspectSkillInstallation(inspect.request)).rejects.toMatchObject({
      message: structuredError.message,
      code: structuredError.code,
      data: golden.inspectionError
    })
  })

  it('rejects malformed structured error data at the JSON-RPC boundary', async () => {
    const inspect = golden.inspectCases[0]
    if (!inspect) throw new Error('The workflow fixture must contain an inspection case')
    rpcRequest.mockRejectedValueOnce(
      Object.assign(new Error('Malformed inspection error'), {
        code: -32011,
        data: { type: 'skillInspection', phase: 'inspect', code: 'rateLimited' }
      })
    )

    await expect(new CoreServer().inspectSkillInstallation(inspect.request)).rejects.toThrow(
      'Skill inspection error data'
    )
  })

  it('validates management errors before preserving them for the IPC envelope', async () => {
    const listError = golden.managementErrors.find((error) => error.operation === 'list')
    const setEnabledError = golden.managementErrors.find(
      (error) => error.operation === 'setEnabled' && error.code === 'stateConflict'
    )
    if (!listError || !setEnabledError) {
      throw new Error('The workflow fixture must cover list and setEnabled errors')
    }
    const cases = [
      {
        data: listError,
        invoke: (server: CoreServer): Promise<unknown> =>
          server.listSkillManagement(golden.management.listRequest)
      },
      {
        data: setEnabledError,
        invoke: (server: CoreServer): Promise<unknown> =>
          server.setSkillEnabled(golden.management.setEnabledRequest)
      }
    ]
    const server = new CoreServer()

    for (const testCase of cases) {
      rpcRequest.mockRejectedValueOnce(
        Object.assign(new Error(testCase.data.message), {
          code: -32012,
          data: testCase.data
        })
      )
      await expect(testCase.invoke(server)).rejects.toMatchObject({
        message: testCase.data.message,
        code: -32012,
        data: testCase.data
      })
    }
  })

  it('rejects malformed management error data at the JSON-RPC boundary', async () => {
    rpcRequest.mockRejectedValueOnce(
      Object.assign(new Error('Malformed management error'), {
        code: -32012,
        data: {
          type: 'skillManagement',
          operation: 'setEnabled',
          code: 'stateConflict',
          message: 'Refresh the management inventory.'
        }
      })
    )

    await expect(
      new CoreServer().setSkillEnabled(golden.management.setEnabledRequest)
    ).rejects.toThrow('Skill management error data')
  })

  it('validates changed notifications and drops malformed payloads', () => {
    let notificationHandler: ((params: unknown) => void) | undefined
    const unsubscribe = vi.fn()
    rpcOnNotification.mockImplementation(
      (_method: string, handler: (params: unknown) => void): (() => void) => {
        notificationHandler = handler
        return unsubscribe
      }
    )
    const changedHandler = vi.fn()
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    const server = new CoreServer()

    const stop = server.onSkillsChanged(changedHandler)
    expect(rpcOnNotification).toHaveBeenCalledWith('skills.changed', expect.any(Function))

    notificationHandler?.(golden.management.changed)
    notificationHandler?.({ ...golden.management.changed, reason: 'unknown' })

    expect(changedHandler).toHaveBeenCalledTimes(1)
    expect(changedHandler).toHaveBeenCalledWith(golden.management.changed)
    expect(warn).toHaveBeenCalledWith(
      'Ignored invalid skills.changed notification',
      expect.any(Error)
    )

    stop()
    expect(unsubscribe).toHaveBeenCalledOnce()
    warn.mockRestore()
  })
})
