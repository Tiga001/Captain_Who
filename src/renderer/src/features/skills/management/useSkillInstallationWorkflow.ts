// Renderer skills installation state machine: owns source selection, frozen preview, and commit.
import { useCallback, useEffect, useRef, useState } from 'react'
import type {
  SkillAcquisitionSource,
  SkillInstallationCommitOutput,
  SkillInstallationPreview,
  SkillManagementEntry
} from '@mycopilot/protocol'
import {
  EMPTY_GITHUB_INSTALLATION_FORM,
  parseGitHubInstallationSource,
  type GitHubInstallationFormValue
} from './skillInstallationSource'
import { getSkillOperationErrorDetails, isExpiredSkillPreviewError } from './skillManagementErrors'
import {
  cancelSkillPreparation,
  commitSkillInstallation,
  inspectSkillInstallation,
  selectSkillInstallationDirectory
} from './skillsManagementClient'

export type SkillInstallationContext =
  { operation: 'install' } | { operation: 'update'; entry: SkillManagementEntry }

export type SkillInstallationWorkflowState =
  | { status: 'idle' }
  | { context: SkillInstallationContext; status: 'choosingSource' }
  | {
      context: SkillInstallationContext
      fieldError: 'repository' | 'reference' | null
      form: GitHubInstallationFormValue
      status: 'githubSource'
    }
  | {
      context: SkillInstallationContext
      preparationId: string
      source: SkillAcquisitionSource
      status: 'inspecting'
    }
  | {
      acceptedIssueIds: readonly string[]
      acquisitionSource: SkillAcquisitionSource
      context: SkillInstallationContext
      errorMessage: string | null
      preview: SkillInstallationPreview
      status: 'preview'
    }
  | {
      acceptedIssueIds: readonly string[]
      acquisitionSource: SkillAcquisitionSource
      context: SkillInstallationContext
      preview: SkillInstallationPreview
      status: 'committing'
    }
  | {
      context: SkillInstallationContext
      expired: boolean
      message: string
      preparationId?: string
      retry?: {
        mode: 'samePreparation' | 'newPreparation'
        source: SkillAcquisitionSource
      }
      status: 'error'
    }

interface UseSkillInstallationWorkflowOptions {
  onCommitted: (output: SkillInstallationCommitOutput) => void | Promise<void>
  onCommitMayHaveSucceeded: () => void | Promise<void>
}

const IDLE_STATE: SkillInstallationWorkflowState = { status: 'idle' }

export function useSkillInstallationWorkflow({
  onCommitted,
  onCommitMayHaveSucceeded
}: UseSkillInstallationWorkflowOptions) {
  const [state, setState] = useState<SkillInstallationWorkflowState>(IDLE_STATE)
  const stateRef = useRef(state)
  const mountedRef = useRef(true)
  const operationSequenceRef = useRef(0)

  useEffect(() => {
    stateRef.current = state
  }, [state])

  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
      operationSequenceRef.current += 1
      const preparationId = getPreparationId(stateRef.current)
      if (preparationId) cancelPreparationBestEffort(preparationId)
    }
  }, [])

  const startInstall = useCallback(() => {
    operationSequenceRef.current += 1
    setState({ context: { operation: 'install' }, status: 'choosingSource' })
  }, [])

  const startUpdate = useCallback((entry: SkillManagementEntry) => {
    operationSequenceRef.current += 1
    setState({ context: { entry, operation: 'update' }, status: 'choosingSource' })
  }, [])

  const inspectSource = useCallback(
    async (
      context: SkillInstallationContext,
      source: SkillAcquisitionSource,
      preparationId: string = crypto.randomUUID()
    ) => {
      const operationSequence = operationSequenceRef.current + 1
      operationSequenceRef.current = operationSequence
      setState({ context, preparationId, source, status: 'inspecting' })

      try {
        const preview = await inspectSkillInstallation({
          preparationId,
          intent:
            context.operation === 'install'
              ? { operation: 'install' }
              : {
                  operation: 'update',
                  skillId: context.entry.id,
                  expectedInstallationRevision: requireInstallationRevision(context.entry)
                },
          source
        })
        if (!mountedRef.current || operationSequenceRef.current !== operationSequence) {
          cancelPreparationBestEffort(preparationId)
          return
        }
        setState({
          acceptedIssueIds: [],
          acquisitionSource: source,
          context,
          errorMessage: null,
          preview,
          status: 'preview'
        })
      } catch (error) {
        if (!mountedRef.current || operationSequenceRef.current !== operationSequence) return
        const details = getSkillOperationErrorDetails(error)
        const expired = isExpiredSkillPreviewError(details)
        const retrySamePreparation =
          details.recovery === 'retrySamePreparation' || details.recovery === 'retryLater'
        setState({
          context,
          expired,
          message: details.message,
          preparationId,
          ...(retrySamePreparation || expired
            ? {
                retry: {
                  mode: retrySamePreparation
                    ? ('samePreparation' as const)
                    : ('newPreparation' as const),
                  source
                }
              }
            : {}),
          status: 'error'
        })
      }
    },
    []
  )

  const chooseLocalDirectory = useCallback(async () => {
    const current = stateRef.current
    if (current.status !== 'choosingSource') return
    try {
      const directory = await selectSkillInstallationDirectory()
      if (!directory || !mountedRef.current || stateRef.current !== current) return
      await inspectSource(current.context, { directory, kind: 'localDirectory' })
    } catch (error) {
      if (!mountedRef.current || stateRef.current !== current) return
      setState({
        context: current.context,
        expired: false,
        message: getSkillOperationErrorDetails(error).message,
        status: 'error'
      })
    }
  }, [inspectSource])

  const chooseGitHub = useCallback(() => {
    const current = stateRef.current
    if (current.status !== 'choosingSource') return
    setState({
      context: current.context,
      fieldError: null,
      form: EMPTY_GITHUB_INSTALLATION_FORM,
      status: 'githubSource'
    })
  }, [])

  const updateGitHubForm = useCallback((patch: Partial<GitHubInstallationFormValue>) => {
    setState((current) =>
      current.status === 'githubSource'
        ? { ...current, fieldError: null, form: { ...current.form, ...patch } }
        : current
    )
  }, [])

  const inspectGitHub = useCallback(async () => {
    const current = stateRef.current
    if (current.status !== 'githubSource') return
    const parsed = parseGitHubInstallationSource(current.form)
    if (!parsed.ok) {
      setState({ ...current, fieldError: parsed.field })
      return
    }
    await inspectSource(current.context, parsed.source)
  }, [inspectSource])

  const toggleAcknowledgement = useCallback((issueId: string) => {
    setState((current) => {
      if (current.status !== 'preview') return current
      const accepted = new Set(current.acceptedIssueIds)
      if (accepted.has(issueId)) accepted.delete(issueId)
      else accepted.add(issueId)
      return { ...current, acceptedIssueIds: [...accepted], errorMessage: null }
    })
  }, [])

  const commit = useCallback(async () => {
    const current = stateRef.current
    if (current.status !== 'preview') return
    if (Date.now() >= current.preview.expiresAtUnixMs) {
      setState({
        context: current.context,
        expired: true,
        message: 'previewExpired',
        preparationId: current.preview.preparationId,
        retry: { mode: 'newPreparation', source: current.acquisitionSource },
        status: 'error'
      })
      return
    }
    const requiredIssueIds = current.preview.compatibility.issues
      .filter((issue) => issue.requiresAcknowledgement)
      .map((issue) => issue.id)
    if (!requiredIssueIds.every((issueId) => current.acceptedIssueIds.includes(issueId))) return

    const operationSequence = operationSequenceRef.current + 1
    operationSequenceRef.current = operationSequence
    setState({ ...current, status: 'committing' })
    let output: SkillInstallationCommitOutput
    try {
      output = await commitSkillInstallation({
        acceptedIssueIds: [...current.acceptedIssueIds],
        preparationId: current.preview.preparationId,
        previewRevision: current.preview.previewRevision
      })
    } catch (error) {
      if (!mountedRef.current || operationSequenceRef.current !== operationSequence) return
      const details = getSkillOperationErrorDetails(error)
      if (details.commitMayHaveSucceeded) {
        // The preview may already have been consumed. Never offer the same commit again; refresh
        // from authoritative server state and let the user confirm the resulting inventory.
        setState(IDLE_STATE)
        await onCommitMayHaveSucceeded()
        return
      }
      if (isExpiredSkillPreviewError(details)) {
        setState({
          context: current.context,
          expired: true,
          message: details.message,
          preparationId: current.preview.preparationId,
          retry: { mode: 'newPreparation', source: current.acquisitionSource },
          status: 'error'
        })
        return
      }
      setState({ ...current, errorMessage: details.message, status: 'preview' })
      return
    }

    if (!mountedRef.current || operationSequenceRef.current !== operationSequence) return

    // A successful commit consumes the frozen preview. Close it before refreshing the list so a
    // UI refresh failure can never expose a stale preview that would invite a duplicate commit.
    setState(IDLE_STATE)
    await onCommitted(output)
  }, [onCommitMayHaveSucceeded, onCommitted])

  const backToSource = useCallback(() => {
    const current = stateRef.current
    if (current.status === 'idle' || current.status === 'committing') return
    operationSequenceRef.current += 1
    const preparationId = getPreparationId(current)
    if (preparationId) cancelPreparationBestEffort(preparationId)
    setState({ context: current.context, status: 'choosingSource' })
  }, [])

  const retryInspection = useCallback(async () => {
    const current = stateRef.current
    if (current.status !== 'error' || !current.retry || !current.preparationId) return
    if (current.retry.mode === 'newPreparation') {
      cancelPreparationBestEffort(current.preparationId)
    }
    await inspectSource(
      current.context,
      current.retry.source,
      current.retry.mode === 'samePreparation' ? current.preparationId : undefined
    )
  }, [inspectSource])

  const close = useCallback(() => {
    const current = stateRef.current
    if (current.status === 'idle' || current.status === 'committing') {
      return
    }
    operationSequenceRef.current += 1
    const preparationId = getPreparationId(current)
    if (preparationId) cancelPreparationBestEffort(preparationId)
    setState(IDLE_STATE)
  }, [])

  return {
    backToSource,
    chooseGitHub,
    chooseLocalDirectory,
    close,
    commit,
    inspectGitHub,
    retryInspection,
    startInstall,
    startUpdate,
    state,
    toggleAcknowledgement,
    updateGitHubForm
  }
}

function requireInstallationRevision(entry: SkillManagementEntry): string {
  if (!entry.installationRevision) {
    throw new Error('Managed Skill is missing its installation revision')
  }
  return entry.installationRevision
}

function getPreparationId(state: SkillInstallationWorkflowState): string | undefined {
  if (state.status === 'inspecting' || state.status === 'error') return state.preparationId
  if (state.status === 'preview' || state.status === 'committing') {
    return state.preview.preparationId
  }
  return undefined
}

function cancelPreparationBestEffort(preparationId: string): void {
  void cancelSkillPreparation({ preparationId }).catch((error: unknown) => {
    console.warn('Failed to cancel Skill installation preparation', error)
  })
}
