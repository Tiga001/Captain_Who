// Renderer skills management domain: validates public GitHub sources before Host API inspection.
import type { SkillAcquisitionSource, SkillGitHubReference } from '@mycopilot/protocol'

export type GitHubReferenceKind = 'defaultBranch' | 'named' | 'commit'

export interface GitHubInstallationFormValue {
  referenceKind: GitHubReferenceKind
  referenceValue: string
  repository: string
  subdirectory: string
}

export type GitHubSourceParseResult =
  | { ok: true; source: Extract<SkillAcquisitionSource, { kind: 'githubRepository' }> }
  | { ok: false; field: 'repository' | 'reference'; reason: 'invalid' | 'required' }

export const EMPTY_GITHUB_INSTALLATION_FORM: GitHubInstallationFormValue = {
  referenceKind: 'defaultBranch',
  referenceValue: '',
  repository: '',
  subdirectory: ''
}

export function parseGitHubInstallationSource(
  value: GitHubInstallationFormValue
): GitHubSourceParseResult {
  const repository = parseGitHubRepository(value.repository)
  if (!repository) return { field: 'repository', ok: false, reason: 'invalid' }

  const reference = parseGitHubReference(value.referenceKind, value.referenceValue)
  if (!reference.ok) return reference

  const subdirectory = value.subdirectory.trim().replace(/^\/+|\/+$/g, '')
  return {
    ok: true,
    source: {
      kind: 'githubRepository',
      owner: repository.owner,
      repository: repository.repository,
      reference: reference.value,
      ...(subdirectory ? { subdirectory } : {})
    }
  }
}

function parseGitHubRepository(input: string): { owner: string; repository: string } | null {
  const trimmed = input.trim()
  let repositoryPath = trimmed

  if (/^https?:\/\//i.test(trimmed)) {
    let url: URL
    try {
      url = new URL(trimmed)
    } catch {
      return null
    }
    if (url.protocol !== 'https:' || url.hostname.toLowerCase() !== 'github.com') return null
    if (url.search || url.hash) return null
    repositoryPath = url.pathname.replace(/^\/+|\/+$/g, '')
  }

  const parts = repositoryPath.split('/')
  if (parts.length !== 2) return null
  const owner = parts[0]?.trim() ?? ''
  const repository = (parts[1]?.trim() ?? '').replace(/\.git$/i, '')
  if (!/^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$/.test(owner)) return null
  if (!/^[A-Za-z0-9._-]+$/.test(repository) || repository === '.' || repository === '..') {
    return null
  }
  return { owner, repository }
}

function parseGitHubReference(
  kind: GitHubReferenceKind,
  rawValue: string
):
  | { ok: true; value: SkillGitHubReference }
  | { ok: false; field: 'reference'; reason: 'invalid' | 'required' } {
  const value = rawValue.trim()
  if (kind === 'defaultBranch') return { ok: true, value: { kind: 'defaultBranch' } }
  if (!value) return { field: 'reference', ok: false, reason: 'required' }
  if (kind === 'commit') {
    return /^[0-9a-fA-F]{7,64}$/.test(value)
      ? { ok: true, value: { kind: 'commit', sha: value } }
      : { field: 'reference', ok: false, reason: 'invalid' }
  }
  return value.length <= 255 && !/\s/.test(value)
    ? { ok: true, value: { kind: 'named', value } }
    : { field: 'reference', ok: false, reason: 'invalid' }
}
