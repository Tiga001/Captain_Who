import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import {
  parseSkillAcquisitionSource,
  parseSkillSourceResolutionErrorData,
  parseSkillsCancelSourceResolutionInput,
  parseSkillsCancelSourceResolutionOutput,
  parseSkillsResolveInstallationSourceInput,
  parseSkillsResolveInstallationSourceOutput,
  SKILL_SOURCE_RESOLUTION_ERROR_CODE,
  SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION,
  SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD,
  SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD
} from '@mycopilot/protocol'
import type {
  SkillAcquisitionSource,
  SkillResolvedGitHubRepositorySource
} from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'

interface SkillSourceResolutionGolden {
  schemaVersion: number
  method: string
  cases: Array<{ name: string; request: unknown; response: unknown }>
  cancel: {
    method: string
    cases: Array<{ request: unknown; response: unknown }>
  }
  errors: unknown[]
}

const golden = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/protocol/fixtures/skill-source-resolution-v2.json'),
    'utf8'
  )
) as SkillSourceResolutionGolden

function cloneJson<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T
}

describe('Skill source resolution protocol', () => {
  it('keeps the stable method, schema, and error identifiers', () => {
    expect(SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD).toBe('skills.resolveInstallationSource')
    expect(SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION).toBe(2)
    expect(SKILL_SOURCE_RESOLUTION_ERROR_CODE).toBe(-32013)
    expect(golden.method).toBe(SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD)
    expect(golden.schemaVersion).toBe(SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION)
    expect(SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD).toBe('skills.cancelSourceResolution')
    expect(golden.cancel.method).toBe(SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD)
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
    for (const testCase of golden.cancel.cases) {
      expect(parseSkillsCancelSourceResolutionInput(testCase.request)).toEqual(testCase.request)
      expect(parseSkillsCancelSourceResolutionOutput(testCase.response)).toEqual(testCase.response)
    }
  })

  it('records valid opaque hashes for candidates produced by the GitHub resolver', () => {
    for (const testCase of golden.cases) {
      const response = testCase.response as {
        candidates: Array<{
          candidateId: string
          acquisition: { candidateId: string }
          package: { fileCount: number; formatVersion: number; packageRevision: string }
        }>
      }
      for (const candidate of response.candidates) {
        expect(candidate.candidateId).toMatch(/^[0-9a-f]{64}$/)
        expect(candidate.acquisition.candidateId).toBe(candidate.candidateId)
        expect(candidate.package.packageRevision).toMatch(
          new RegExp(`^skill-package-sha256-v${candidate.package.formatVersion}:[0-9a-f]{64}$`)
        )
        expect(candidate.package.formatVersion === 1).toBe(candidate.package.fileCount === 1)
      }
    }
  })

  it('preserves tracking URLs and stable provider-generic resolution errors', () => {
    const exact = golden.cases.find(
      (testCase) => testCase.name === 'an exact Skill URL resolves without a selection step'
    )
    const repository = golden.cases.find(
      (testCase) => testCase.name === 'a repository containing multiple Skills requires selection'
    )
    if (!exact || !repository) {
      throw new Error('The source resolution fixture must cover exact and repository URLs')
    }

    expect((exact.response as { canonicalUrl: string }).canonicalUrl).toBe(
      (exact.request as { locator: { url: string } }).locator.url
    )
    expect((repository.response as { canonicalUrl: string }).canonicalUrl).toBe(
      (repository.request as { locator: { url: string } }).locator.url
    )

    const rateLimited = golden.errors
      .map(parseSkillSourceResolutionErrorData)
      .find((error) => error.code === 'rateLimited')
    expect(rateLimited).toMatchObject({
      provider: 'github',
      message: 'The Skill source provider temporarily refused the resolution request.'
    })
  })

  it('preserves actionable lifecycle failures instead of flattening them to unavailable', () => {
    const lifecycleErrors = golden.errors
      .map(parseSkillSourceResolutionErrorData)
      .filter((error) =>
        [
          'resolutionNotFoundOrExpired',
          'resolutionConsumed',
          'candidateNotFound',
          'cancelled'
        ].includes(error.code)
      )

    expect(lifecycleErrors.map(({ code, recovery }) => ({ code, recovery }))).toEqual([
      { code: 'resolutionNotFoundOrExpired', recovery: 'startNewResolution' },
      { code: 'resolutionConsumed', recovery: 'startNewResolution' },
      { code: 'candidateNotFound', recovery: 'retrySameResolution' },
      { code: 'cancelled', recovery: 'startNewResolution' }
    ])
  })

  it('requires every candidate source to be pinned to the response commit', () => {
    const shortCommit = cloneJson(golden.cases[0].response) as Record<string, unknown>
    shortCommit.resolvedCommit = 'abc123'
    expect(() => parseSkillsResolveInstallationSourceOutput(shortCommit)).toThrow(
      '40-character hexadecimal SHA'
    )

    const mismatchedCommit = cloneJson(golden.cases[0].response) as {
      candidates: Array<{ source: { resolvedCommit: string } }>
    }
    mismatchedCommit.candidates[0].source.resolvedCommit =
      'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
    expect(() => parseSkillsResolveInstallationSourceOutput(mismatchedCommit)).toThrow(
      'must match response.resolvedCommit'
    )

    const mismatchedTrackingCommit = cloneJson(golden.cases[0].response) as {
      candidates: Array<{ source: { reference: Record<string, unknown> } }>
    }
    mismatchedTrackingCommit.candidates[0].source.reference = {
      kind: 'commit',
      sha: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
    }
    expect(() => parseSkillsResolveInstallationSourceOutput(mismatchedTrackingCommit)).toThrow(
      'commit reference sha must match resolvedCommit'
    )

    const missingCandidatePin = cloneJson(golden.cases[0].response) as {
      candidates: Array<{ source: { resolvedCommit?: string } }>
    }
    delete missingCandidatePin.candidates[0].source.resolvedCommit
    expect(() => parseSkillsResolveInstallationSourceOutput(missingCandidatePin)).toThrow(
      'candidate.source.resolvedCommit'
    )
  })

  it('validates one-time candidate handoff identities and expiry', () => {
    const mismatchedResolution = cloneJson(golden.cases[0].response) as {
      candidates: Array<{ acquisition: { resolutionId: string } }>
    }
    mismatchedResolution.candidates[0].acquisition.resolutionId =
      '99999999-9999-4999-8999-999999999999'
    expect(() => parseSkillsResolveInstallationSourceOutput(mismatchedResolution)).toThrow(
      'resolutionId must match response.resolutionId'
    )

    const mismatchedCandidate = cloneJson(golden.cases[0].response) as {
      candidates: Array<{ acquisition: { candidateId: string } }>
    }
    mismatchedCandidate.candidates[0].acquisition.candidateId = 'different-candidate'
    expect(() => parseSkillsResolveInstallationSourceOutput(mismatchedCandidate)).toThrow(
      'candidateId must match the enclosing candidateId'
    )

    for (const invalidId of [
      '00000000-0000-0000-0000-000000000000',
      '11111111-1111-4111-8111-11111111111A',
      'not-a-uuid'
    ]) {
      const invalidResponseId = cloneJson(golden.cases[0].response) as Record<string, unknown>
      invalidResponseId.resolutionId = invalidId
      expect(() => parseSkillsResolveInstallationSourceOutput(invalidResponseId)).toThrow(
        'canonical non-nil lower-case UUID'
      )
    }

    const invalidExpiry = cloneJson(golden.cases[0].response) as Record<string, unknown>
    invalidExpiry.expiresAtUnixMs = 0
    expect(() => parseSkillsResolveInstallationSourceOutput(invalidExpiry)).toThrow(
      'expiresAtUnixMs'
    )
  })

  it('keeps presentation pins out of ordinary acquisition authority', () => {
    const commit = '0123456789abcdef0123456789abcdef01234567'
    expect(
      parseSkillAcquisitionSource({
        kind: 'githubRepository',
        owner: 'openai',
        repository: 'skills',
        reference: { kind: 'named', value: 'main' },
        subdirectory: 'skills/auditor'
      })
    ).toEqual({
      kind: 'githubRepository',
      owner: 'openai',
      repository: 'skills',
      reference: { kind: 'named', value: 'main' },
      subdirectory: 'skills/auditor'
    })

    const presentationSource: SkillResolvedGitHubRepositorySource = {
      kind: 'githubRepository',
      owner: 'openai',
      repository: 'skills',
      reference: { kind: 'named', value: 'main' },
      resolvedCommit: commit,
      subdirectory: 'skills/auditor'
    }
    // @ts-expect-error Presentation metadata is deliberately not acquisition authority.
    const invalidAcquisition: SkillAcquisitionSource = presentationSource
    expect(() => parseSkillAcquisitionSource(invalidAcquisition)).toThrow(
      'unexpected field resolvedCommit'
    )
    expect(() =>
      parseSkillAcquisitionSource({
        kind: 'resolvedCandidate',
        resolutionId: '00000000-0000-0000-0000-000000000000',
        candidateId: 'candidate'
      })
    ).toThrow('canonical non-nil lower-case UUID')
  })

  it('strictly parses idempotent cancellation requests and outcomes', () => {
    expect(() =>
      parseSkillsCancelSourceResolutionInput({
        resolutionId: '00000000-0000-0000-0000-000000000000'
      })
    ).toThrow('canonical non-nil lower-case UUID')
    expect(() =>
      parseSkillsCancelSourceResolutionInput({
        resolutionId: '11111111-1111-4111-8111-111111111111',
        candidateId: 'must-not-cross-the-boundary'
      })
    ).toThrow('unexpected field candidateId')
    expect(() =>
      parseSkillsCancelSourceResolutionOutput({
        schemaVersion: 2,
        resolutionId: '11111111-1111-4111-8111-111111111111',
        outcome: 'forgotten'
      })
    ).toThrow('outcome')
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
      candidates: Array<{ candidateId: string; acquisition: { candidateId: string } }>
    }
    duplicateIds.candidates[1].candidateId = duplicateIds.candidates[0].candidateId
    duplicateIds.candidates[1].acquisition.candidateId = duplicateIds.candidates[0].candidateId
    expect(() => parseSkillsResolveInstallationSourceOutput(duplicateIds)).toThrow(
      'candidateId values must be unique'
    )
  })

  it('rejects unknown request, response, candidate, and error fields or enums', () => {
    expect(() =>
      parseSkillsResolveInstallationSourceInput({
        resolutionId: '00000000-0000-0000-0000-000000000000',
        locator: { kind: 'url', url: 'https://github.com/openai/example-skills' }
      })
    ).toThrow('canonical non-nil lower-case UUID')
    expect(() =>
      parseSkillsResolveInstallationSourceInput({
        resolutionId: '11111111-1111-4111-8111-111111111111',
        locator: { kind: 'git', url: 'https://github.com/openai/example-skills' }
      })
    ).toThrow('unknown kind git')
    expect(() =>
      parseSkillsResolveInstallationSourceInput({
        resolutionId: '11111111-1111-4111-8111-111111111111',
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
    unknownSchema.schemaVersion = 1
    expect(() => parseSkillsResolveInstallationSourceOutput(unknownSchema)).toThrow(
      'unsupported schema version 1'
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
