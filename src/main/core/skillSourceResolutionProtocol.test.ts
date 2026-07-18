import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import {
  parseSkillSourceResolutionErrorData,
  parseSkillsResolveInstallationSourceInput,
  parseSkillsResolveInstallationSourceOutput,
  SKILL_SOURCE_RESOLUTION_ERROR_CODE,
  SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION,
  SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD
} from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'

interface SkillSourceResolutionGolden {
  schemaVersion: number
  method: string
  cases: Array<{ name: string; request: unknown; response: unknown }>
  errors: unknown[]
}

const golden = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/protocol/fixtures/skill-source-resolution-v1.json'),
    'utf8'
  )
) as SkillSourceResolutionGolden

function cloneJson<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T
}

describe('Skill source resolution protocol', () => {
  it('keeps the stable method, schema, and error identifiers', () => {
    expect(SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD).toBe('skills.resolveInstallationSource')
    expect(SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION).toBe(1)
    expect(SKILL_SOURCE_RESOLUTION_ERROR_CODE).toBe(-32013)
    expect(golden.method).toBe(SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD)
    expect(golden.schemaVersion).toBe(SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION)
  })

  it('parses the same requests, responses, and errors as Rust', () => {
    for (const testCase of golden.cases) {
      expect(parseSkillsResolveInstallationSourceInput(testCase.request)).toEqual(testCase.request)
      expect(parseSkillsResolveInstallationSourceOutput(testCase.response)).toEqual(
        testCase.response
      )
    }
    for (const error of golden.errors) {
      expect(parseSkillSourceResolutionErrorData(error)).toEqual(error)
    }
  })

  it('requires every candidate source to be pinned to the response commit', () => {
    const shortCommit = cloneJson(golden.cases[0].response) as Record<string, unknown>
    shortCommit.resolvedCommit = 'abc123'
    expect(() => parseSkillsResolveInstallationSourceOutput(shortCommit)).toThrow(
      '40-character hexadecimal SHA'
    )

    const mismatchedCommit = cloneJson(golden.cases[0].response) as {
      candidates: Array<{ source: { reference: { sha: string } } }>
    }
    mismatchedCommit.candidates[0].source.reference.sha = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
    expect(() => parseSkillsResolveInstallationSourceOutput(mismatchedCommit)).toThrow(
      'must match response.resolvedCommit'
    )

    const mutableReference = cloneJson(golden.cases[0].response) as {
      candidates: Array<{ source: { reference: Record<string, unknown> } }>
    }
    mutableReference.candidates[0].source.reference = { kind: 'defaultBranch' }
    expect(() => parseSkillsResolveInstallationSourceOutput(mutableReference)).toThrow(
      'kind must be commit'
    )
  })

  it('enforces outcome cardinality and unique stable candidate ids', () => {
    const resolvedWithMany = cloneJson(golden.cases[1].response) as Record<string, unknown>
    resolvedWithMany.outcome = 'resolved'
    expect(() => parseSkillsResolveInstallationSourceOutput(resolvedWithMany)).toThrow(
      'resolved outcome requires exactly one candidate'
    )

    const selectionWithOne = cloneJson(golden.cases[0].response) as Record<string, unknown>
    selectionWithOne.outcome = 'selectionRequired'
    expect(() => parseSkillsResolveInstallationSourceOutput(selectionWithOne)).toThrow(
      'selectionRequired outcome requires at least two candidates'
    )

    const duplicateIds = cloneJson(golden.cases[1].response) as {
      candidates: Array<{ candidateId: string }>
    }
    duplicateIds.candidates[1].candidateId = duplicateIds.candidates[0].candidateId
    expect(() => parseSkillsResolveInstallationSourceOutput(duplicateIds)).toThrow(
      'candidateId values must be unique'
    )
  })

  it('rejects unknown request, response, candidate, and error fields or enums', () => {
    expect(() =>
      parseSkillsResolveInstallationSourceInput({
        locator: { kind: 'git', url: 'https://github.com/openai/example-skills' }
      })
    ).toThrow('unknown kind git')
    expect(() =>
      parseSkillsResolveInstallationSourceInput({
        locator: {
          kind: 'url',
          url: 'https://github.com/openai/example-skills',
          credential: 'must-not-cross-the-boundary'
        }
      })
    ).toThrow('unexpected field credential')

    const unknownOutcome = cloneJson(golden.cases[0].response) as Record<string, unknown>
    unknownOutcome.outcome = 'installed'
    expect(() => parseSkillsResolveInstallationSourceOutput(unknownOutcome)).toThrow('outcome')

    const unknownProvider = cloneJson(golden.cases[0].response) as Record<string, unknown>
    unknownProvider.provider = 'gitlab'
    expect(() => parseSkillsResolveInstallationSourceOutput(unknownProvider)).toThrow('provider')

    const unknownSchema = cloneJson(golden.cases[0].response) as Record<string, unknown>
    unknownSchema.schemaVersion = 2
    expect(() => parseSkillsResolveInstallationSourceOutput(unknownSchema)).toThrow(
      'unsupported schema version 2'
    )

    const candidateWithExtraField = cloneJson(golden.cases[0].response) as {
      candidates: Array<Record<string, unknown>>
    }
    candidateWithExtraField.candidates[0].localDirectory = '/private/skill'
    expect(() => parseSkillsResolveInstallationSourceOutput(candidateWithExtraField)).toThrow(
      'unexpected field localDirectory'
    )

    expect(() =>
      parseSkillSourceResolutionErrorData({
        ...(golden.errors[0] as Record<string, unknown>),
        code: 'repositoryMoved'
      })
    ).toThrow('code')
    for (const [field, value] of [
      ['phase', 'install'],
      ['recovery', 'retrySamePreparation'],
      ['provider', 'gitlab']
    ] as const) {
      expect(() =>
        parseSkillSourceResolutionErrorData({
          ...(golden.errors[1] as Record<string, unknown>),
          [field]: value
        })
      ).toThrow(field)
    }
    expect(() =>
      parseSkillSourceResolutionErrorData({
        ...(golden.errors[0] as Record<string, unknown>),
        internalUrl: 'https://example.invalid/secret'
      })
    ).toThrow('unexpected field internalUrl')
  })
})
