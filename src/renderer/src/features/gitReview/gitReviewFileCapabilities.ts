import type { GitReviewFile } from '@mycopilot/protocol'

/** Only two-sided working-tree files can hydrate compact patches with complete source text. */
export function canHydrateGitReviewFile(status: GitReviewFile['status']): boolean {
  return status === 'modified' || status === 'renamed' || status === 'copied'
}
