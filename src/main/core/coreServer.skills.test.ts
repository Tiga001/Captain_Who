import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import type {
  SkillInstallationErrorData,
  SkillInstallationOperation,
  SkillMutationOutput,
  SkillsInstallLocalInput,
  SkillsUninstallInput,
  SkillsUpdateLocalInput
} from '@mycopilot/protocol'
import { parseSkillMutationOutput, SKILL_MUTATION_SCHEMA_VERSION } from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const rpcRequest = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = rpcRequest
  }
}))

import { CoreServer } from './coreServer'

const installationId = '018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d'
const skillId = `installed:user:${installationId}`
const sharedMutationGolden = JSON.parse(
  readFileSync(resolve(process.cwd(), 'packages/protocol/fixtures/skill-mutation-v1.json'), 'utf8')
) as {
  schemaVersion: number
  cases: Array<{ operation: SkillInstallationOperation; response: unknown }>
}

describe('CoreServer Skill mutation client', () => {
  beforeEach(() => {
    rpcRequest.mockReset()
  })

  it('routes local install, local update, and uninstall through their stable RPC methods', async () => {
    const installInput = {
      installationId,
      directory: '/tmp/local-skill'
    } satisfies SkillsInstallLocalInput
    const updateInput = {
      skillId,
      expectedRevision: 'skill-package-sha256-v1:old',
      directory: '/tmp/local-skill'
    } satisfies SkillsUpdateLocalInput
    const uninstallInput = {
      skillId,
      expectedRevision: 'skill-package-sha256-v1:new'
    } satisfies SkillsUninstallInput
    const outputs = [
      {
        schemaVersion: SKILL_MUTATION_SCHEMA_VERSION,
        installationId,
        skillId,
        revision: 'skill-package-sha256-v1:old',
        outcome: 'installed'
      },
      {
        schemaVersion: SKILL_MUTATION_SCHEMA_VERSION,
        installationId,
        skillId,
        revision: 'skill-package-sha256-v1:new',
        outcome: 'updated'
      },
      {
        schemaVersion: SKILL_MUTATION_SCHEMA_VERSION,
        installationId,
        skillId,
        outcome: 'uninstalled'
      }
    ] satisfies SkillMutationOutput[]
    for (const output of outputs) rpcRequest.mockResolvedValueOnce(output)
    const server = new CoreServer()

    await expect(server.installLocalSkill(installInput)).resolves.toEqual(outputs[0])
    await expect(server.updateLocalSkill(updateInput)).resolves.toEqual(outputs[1])
    await expect(server.uninstallSkill(uninstallInput)).resolves.toEqual(outputs[2])

    expect(rpcRequest).toHaveBeenNthCalledWith(1, 'skills.installLocal', installInput)
    expect(rpcRequest).toHaveBeenNthCalledWith(2, 'skills.updateLocal', updateInput)
    expect(rpcRequest).toHaveBeenNthCalledWith(3, 'skills.uninstall', uninstallInput)
  })

  it('models indeterminate recovery without exposing the acquisition directory', () => {
    const data = {
      type: 'skillInstallation',
      operation: 'update',
      code: 'commitIndeterminate',
      recovery: 'retrySameRequest',
      message: 'The update may already be visible.',
      commitMayHaveSucceeded: true,
      installationId,
      skillId,
      intendedRevision: 'skill-package-sha256-v1:new',
      expectedRevision: 'skill-package-sha256-v1:old'
    } satisfies SkillInstallationErrorData

    expect(data).not.toHaveProperty('directory')
    expect(data.commitMayHaveSucceeded).toBe(true)
  })

  it('parses the same mutation wire golden serialized by Rust', () => {
    expect(sharedMutationGolden.schemaVersion).toBe(SKILL_MUTATION_SCHEMA_VERSION)
    for (const golden of sharedMutationGolden.cases) {
      expect(parseSkillMutationOutput(golden.response, golden.operation)).toEqual(golden.response)
    }
  })

  it.each([
    [
      'an unsupported schema version',
      {
        schemaVersion: 2,
        installationId,
        skillId,
        revision: 'skill-package-sha256-v1:new',
        outcome: 'installed'
      }
    ],
    [
      'a package outcome without a revision',
      {
        schemaVersion: SKILL_MUTATION_SCHEMA_VERSION,
        installationId,
        skillId,
        outcome: 'updated'
      }
    ],
    [
      'a removal outcome carrying a revision',
      {
        schemaVersion: SKILL_MUTATION_SCHEMA_VERSION,
        installationId,
        skillId,
        revision: 'skill-package-sha256-v1:removed',
        outcome: 'uninstalled'
      }
    ],
    [
      'an update outcome returned by install',
      {
        schemaVersion: SKILL_MUTATION_SCHEMA_VERSION,
        installationId,
        skillId,
        revision: 'skill-package-sha256-v1:new',
        outcome: 'updated'
      }
    ]
  ])('rejects %s at the JSON-RPC boundary', async (_label, output) => {
    rpcRequest.mockResolvedValueOnce(output)
    const server = new CoreServer()

    await expect(
      server.installLocalSkill({ installationId, directory: '/tmp/local-skill' })
    ).rejects.toThrow('Invalid Skill mutation response')
  })
})
