import type {
  GitRepositoryInspection,
  GitReviewCommitList,
  GitReviewCommitListInput,
  GitReviewFileContent,
  GitReviewFileContentInput,
  GitReviewFileDiff,
  GitReviewFileDiffInput,
  GitReviewFileMutation,
  GitReviewFileMutationInput,
  GitReviewSummary,
  GitReviewSummaryInput,
  GitReviewRepositoryContext,
  GitReviewRepositoryContextInput,
  GitTurnDiffSummaries,
  GitTurnDiffSummariesInput,
  WorkspaceFileRequest
} from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'

export function copyGitReviewFilePath(input: WorkspaceFileRequest): Promise<void> {
  return hostClient.workspaceFiles.copyPath(input)
}

export function inspectGitRepository(projectId: string): Promise<GitRepositoryInspection> {
  return hostClient.git.inspectRepository({ projectId })
}

export function getGitReviewSummary(input: GitReviewSummaryInput): Promise<GitReviewSummary> {
  return hostClient.git.getReviewSummary(input)
}

export function getGitReviewRepositoryContext(
  input: GitReviewRepositoryContextInput
): Promise<GitReviewRepositoryContext> {
  return hostClient.git.getReviewRepositoryContext(input)
}

export function listGitReviewCommits(
  input: GitReviewCommitListInput
): Promise<GitReviewCommitList> {
  return hostClient.git.listReviewCommits(input)
}

export function getGitTurnDiffSummaries(
  input: GitTurnDiffSummariesInput
): Promise<GitTurnDiffSummaries> {
  return hostClient.git.getTurnDiffSummaries(input)
}

export function getGitReviewFileDiff(input: GitReviewFileDiffInput): Promise<GitReviewFileDiff> {
  return hostClient.git.getReviewFileDiff(input)
}

export function getGitReviewFileContent(
  input: GitReviewFileContentInput
): Promise<GitReviewFileContent> {
  return hostClient.git.getReviewFileContent(input)
}

export function mutateGitReviewFile(
  input: GitReviewFileMutationInput
): Promise<GitReviewFileMutation> {
  return hostClient.git.mutateReviewFile(input)
}
