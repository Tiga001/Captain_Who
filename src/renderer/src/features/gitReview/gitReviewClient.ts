import type {
  GitRepositoryInspection,
  GitReviewFileDiff,
  GitReviewFileDiffInput,
  GitReviewFileMutation,
  GitReviewFileMutationInput,
  GitReviewSummary,
  GitReviewSummaryInput
} from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'

export function inspectGitRepository(projectId: string): Promise<GitRepositoryInspection> {
  return hostClient.git.inspectRepository({ projectId })
}

export function getGitReviewSummary(input: GitReviewSummaryInput): Promise<GitReviewSummary> {
  return hostClient.git.getReviewSummary(input)
}

export function getGitReviewFileDiff(input: GitReviewFileDiffInput): Promise<GitReviewFileDiff> {
  return hostClient.git.getReviewFileDiff(input)
}

export function mutateGitReviewFile(
  input: GitReviewFileMutationInput
): Promise<GitReviewFileMutation> {
  return hostClient.git.mutateReviewFile(input)
}
