import { describe, expect, it } from 'vitest'
import { parseGitHubInstallationSource } from '../management/skillInstallationSource'

describe('parseGitHubInstallationSource', () => {
  it('maps repository shorthand and reference fields to a structured public GitHub source', () => {
    expect(
      parseGitHubInstallationSource({
        referenceKind: 'named',
        referenceValue: 'release/v2',
        repository: 'openai/codex',
        subdirectory: '/skills/reviewer/'
      })
    ).toEqual({
      ok: true,
      source: {
        kind: 'githubRepository',
        owner: 'openai',
        repository: 'codex',
        reference: { kind: 'named', value: 'release/v2' },
        subdirectory: 'skills/reviewer'
      }
    })
  })

  it('accepts only GitHub repository home URLs and rejects arbitrary download URLs', () => {
    expect(
      parseGitHubInstallationSource({
        referenceKind: 'defaultBranch',
        referenceValue: '',
        repository: 'https://github.com/openai/codex.git',
        subdirectory: ''
      })
    ).toMatchObject({ ok: true })
    expect(
      parseGitHubInstallationSource({
        referenceKind: 'defaultBranch',
        referenceValue: '',
        repository: 'https://example.com/openai/codex.zip',
        subdirectory: ''
      })
    ).toEqual({ field: 'repository', ok: false, reason: 'invalid' })
    expect(
      parseGitHubInstallationSource({
        referenceKind: 'defaultBranch',
        referenceValue: '',
        repository: 'https://github.com/openai/codex/tree/main',
        subdirectory: ''
      })
    ).toEqual({ field: 'repository', ok: false, reason: 'invalid' })
  })

  it('requires a bounded hexadecimal SHA for commit references', () => {
    expect(
      parseGitHubInstallationSource({
        referenceKind: 'commit',
        referenceValue: 'abcdef1',
        repository: 'openai/codex',
        subdirectory: ''
      })
    ).toMatchObject({
      ok: true,
      source: { reference: { kind: 'commit', sha: 'abcdef1' } }
    })
    expect(
      parseGitHubInstallationSource({
        referenceKind: 'commit',
        referenceValue: 'not-a-sha',
        repository: 'openai/codex',
        subdirectory: ''
      })
    ).toEqual({ field: 'reference', ok: false, reason: 'invalid' })
  })
})
