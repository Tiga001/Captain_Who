// Renderer skills installation state machine: preserves backend resolution and preview authority.
import { useCallback, useEffect, useRef, useState } from 'react'
import type {
  SkillInstallationCommitOutput,
  SkillInstallationPreview,
  SkillManagementEntry,
  SkillSourceResolutionCandidate,
  SkillsCommitInstallationInput,
  SkillsInspectInstallationInput,
  SkillsResolveInstallationSourceOutput
} from '@mycopilot/protocol'
import type { TranslationKey } from '../../../config/languageRegistry'
import {
  getSkillOperationErrorDetails,
  getSkillOperationErrorKey,
  type SkillOperationErrorDetails
} from './skillManagementErrors'
import {
  cancelSkillPreparation,
  cancelSkillSourceResolution,
  commitSkillInstallation,
  inspectSkillInstallation,
  resolveSkillInstallationSource,
  selectSkillInstallationDirectory
} from './skillsManagementClient'

export type SkillInstallationContext =
  { operation: 'install' } | { operation: 'update'; entry: SkillManagementEntry }

interface ResolutionTransaction {
  frozenUrl: string
  resolutionId: string
}

type InspectionOrigin =
  ({ kind: 'url' } & ResolutionTransaction) | { kind: 'local' } | { kind: 'installedSource' }

interface InspectionTransaction {
  context: SkillInstallationContext
  input: SkillsInspectInstallationInput
  origin: InspectionOrigin
}

interface PreviewSession {
  acceptedIssueIds: readonly string[]
  errorKey: TranslationKey | null
  inspection: InspectionTransaction
  preview: SkillInstallationPreview
}

interface WorkflowErrorState {
  context: SkillInstallationContext
  details: SkillOperationErrorDetails
  errorKey: TranslationKey
  inspection?: InspectionTransaction
  phase: 'resolve' | 'inspect' | 'commit'
  previewSession?: PreviewSession
  resolution?: ResolutionTransaction
  status: 'error'
}

export type SkillInstallationWorkflowState =
  | { status: 'idle' }
  | {
      context: Extract<SkillInstallationContext, { operation: 'install' }>
      localError: boolean
      status: 'choosingSource'
    }
  | {
      context: Extract<SkillInstallationContext, { operation: 'install' }>
      fieldError: boolean
      status: 'urlInput'
      url: string
    }
  | {
      context: Extract<SkillInstallationContext, { operation: 'install' }>
      resolution: ResolutionTransaction
      status: 'resolving'
    }
  | {
      context: Extract<SkillInstallationContext, { operation: 'install' }>
      output: SkillsResolveInstallationSourceOutput
      resolution: ResolutionTransaction
      status: 'candidates'
    }
  | { inspection: InspectionTransaction; status: 'inspecting' }
  | ({ status: 'preview' } & PreviewSession)
  | ({ status: 'committing' } & PreviewSession)
  | WorkflowErrorState

interface UseSkillInstallationWorkflowOptions {
  onCommitted: (output: SkillInstallationCommitOutput) => void | Promise<void>
  onCommitIndeterminate: (details: SkillOperationErrorDetails) => void | Promise<void>
  onRefreshManagement: () => void | Promise<void>
}

const IDLE_STATE: SkillInstallationWorkflowState = { status: 'idle' }

export function useSkillInstallationWorkflow({
  onCommitted,
  onCommitIndeterminate,
  onRefreshManagement
}: UseSkillInstallationWorkflowOptions) {
  const [state, setState] = useState<SkillInstallationWorkflowState>(IDLE_STATE)
  const stateRef = useRef<SkillInstallationWorkflowState>(IDLE_STATE)
  const mountedRef = useRef(true)
  const operationEpochRef = useRef(0)
  const returnFocusRef = useRef<HTMLElement | null>(null)

  const publish = useCallback((next: SkillInstallationWorkflowState) => {
    stateRef.current = next
    if (mountedRef.current) setState(next)
  }, [])

  const restoreTriggerFocus = useCallback(() => {
    const target = returnFocusRef.current
    returnFocusRef.current = null
    if (!target?.isConnected) return
    window.requestAnimationFrame(() => target.focus({ preventScroll: true }))
  }, [])

  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
      operationEpochRef.current += 1
      releaseStateAuthorities(stateRef.current)
    }
  }, [])

  const inspect = useCallback(
    async (inspection: InspectionTransaction) => {
      const epoch = ++operationEpochRef.current
      publish({ inspection, status: 'inspecting' })

      try {
        const preview = await inspectSkillInstallation(inspection.input)
        if (!mountedRef.current || operationEpochRef.current !== epoch) {
          cancelPreparationBestEffort(inspection.input.preparationId)
          return
        }
        publish({
          acceptedIssueIds: [],
          errorKey: null,
          inspection,
          preview,
          status: 'preview'
        })
      } catch (error) {
        if (!mountedRef.current || operationEpochRef.current !== epoch) return
        const details = getSkillOperationErrorDetails(error)
        publish({
          context: inspection.context,
          details,
          errorKey: publicErrorKey(inspection.origin, details),
          inspection,
          phase: 'inspect',
          status: 'error'
        })
      }
    },
    [publish]
  )

  const resolveUrl = useCallback(
    async (
      context: Extract<SkillInstallationContext, { operation: 'install' }>,
      resolution: ResolutionTransaction
    ) => {
      const epoch = ++operationEpochRef.current
      publish({ context, resolution, status: 'resolving' })

      try {
        const output = await resolveSkillInstallationSource({
          locator: { kind: 'url', url: resolution.frozenUrl },
          resolutionId: resolution.resolutionId
        })
        if (!mountedRef.current || operationEpochRef.current !== epoch) {
          cancelResolutionBestEffort(resolution.resolutionId)
          return
        }
        if (output.outcome === 'resolved') {
          const candidate = output.candidates[0]
          await inspect({
            context,
            input: {
              intent: { operation: 'install' },
              preparationId: crypto.randomUUID(),
              // The candidate authority is opaque and one-time. Never rebuild it from source.
              source: candidate.acquisition
            },
            origin: { ...resolution, kind: 'url' }
          })
          return
        }
        publish({ context, output, resolution, status: 'candidates' })
      } catch (error) {
        if (!mountedRef.current || operationEpochRef.current !== epoch) return
        const details = getSkillOperationErrorDetails(error)
        publish({
          context,
          details,
          errorKey: getSkillOperationErrorKey(details),
          phase: 'resolve',
          resolution,
          status: 'error'
        })
      }
    },
    [inspect, publish]
  )

  const startInstall = useCallback(
    (trigger?: HTMLElement) => {
      returnFocusRef.current =
        trigger ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null)
      operationEpochRef.current += 1
      releaseStateAuthorities(stateRef.current)
      publish({
        context: { operation: 'install' },
        localError: false,
        status: 'choosingSource'
      })
    },
    [publish]
  )

  const startUpdate = useCallback(
    (entry: SkillManagementEntry, trigger?: HTMLElement) => {
      if (!entry.installationRevision) return
      returnFocusRef.current =
        trigger ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null)
      operationEpochRef.current += 1
      releaseStateAuthorities(stateRef.current)
      const context: SkillInstallationContext = { entry, operation: 'update' }
      void inspect({
        context,
        input: {
          intent: {
            expectedInstallationRevision: entry.installationRevision,
            operation: 'update',
            skillId: entry.id
          },
          preparationId: crypto.randomUUID(),
          source: { kind: 'installedSource' }
        },
        origin: { kind: 'installedSource' }
      })
    },
    [inspect]
  )

  const chooseGitHubSource = useCallback(() => {
    const current = stateRef.current
    if (current.status !== 'choosingSource') return
    publish({
      context: current.context,
      fieldError: false,
      status: 'urlInput',
      url: ''
    })
  }, [publish])

  const updateUrl = useCallback(
    (url: string) => {
      const current = stateRef.current
      if (current.status !== 'urlInput') return
      publish({ ...current, fieldError: false, url })
    },
    [publish]
  )

  const submitUrl = useCallback(async () => {
    const current = stateRef.current
    if (current.status !== 'urlInput') return
    const frozenUrl = current.url.trim()
    if (!frozenUrl) {
      publish({ ...current, fieldError: true })
      return
    }
    await resolveUrl(current.context, {
      frozenUrl,
      resolutionId: crypto.randomUUID()
    })
  }, [publish, resolveUrl])

  const chooseCandidate = useCallback(
    async (candidate: SkillSourceResolutionCandidate) => {
      const current = stateRef.current
      if (current.status !== 'candidates') return
      await inspect({
        context: current.context,
        input: {
          intent: { operation: 'install' },
          preparationId: crypto.randomUUID(),
          // candidateId and resolutionId remain opaque; acquisition crosses the boundary intact.
          source: candidate.acquisition
        },
        origin: { ...current.resolution, kind: 'url' }
      })
    },
    [inspect]
  )

  const chooseLocalDirectory = useCallback(async () => {
    const current = stateRef.current
    if (current.status !== 'choosingSource') return
    const pickerEpoch = ++operationEpochRef.current
    try {
      const directory = await selectSkillInstallationDirectory()
      if (!mountedRef.current || operationEpochRef.current !== pickerEpoch || !directory) return
      await inspect({
        context: current.context,
        input: {
          intent: { operation: 'install' },
          preparationId: crypto.randomUUID(),
          source: { directory, kind: 'localDirectory' }
        },
        origin: { kind: 'local' }
      })
    } catch {
      if (!mountedRef.current || operationEpochRef.current !== pickerEpoch) return
      // Native paths are intentionally excluded from UI errors and logs.
      publish({ ...current, localError: true })
    }
  }, [inspect, publish])

  const toggleAcknowledgement = useCallback(
    (issueId: string) => {
      const current = stateRef.current
      if (current.status !== 'preview') return
      const accepted = new Set(current.acceptedIssueIds)
      if (accepted.has(issueId)) accepted.delete(issueId)
      else accepted.add(issueId)
      publish({ ...current, acceptedIssueIds: [...accepted], errorKey: null })
    },
    [publish]
  )

  const commitSession = useCallback(
    async (session: PreviewSession) => {
      const requiredIssueIds = session.preview.compatibility.issues
        .filter((issue) => issue.requiresAcknowledgement)
        .map((issue) => issue.id)
      if (
        Date.now() >= session.preview.expiresAtUnixMs ||
        session.preview.compatibility.status === 'incompatible' ||
        !requiredIssueIds.every((issueId) => session.acceptedIssueIds.includes(issueId))
      ) {
        return
      }

      const input: SkillsCommitInstallationInput = {
        acceptedIssueIds: requiredIssueIds,
        preparationId: session.preview.preparationId,
        previewRevision: session.preview.previewRevision
      }
      const epoch = ++operationEpochRef.current
      publish({ ...session, errorKey: null, status: 'committing' })

      try {
        const output = await commitSkillInstallation(input)
        if (!mountedRef.current || operationEpochRef.current !== epoch) return
        publish(IDLE_STATE)
        restoreTriggerFocus()
        await onCommitted(output)
      } catch (error) {
        if (!mountedRef.current || operationEpochRef.current !== epoch) return
        const details = getSkillOperationErrorDetails(error)
        if (details.code === 'commitIndeterminate' || details.commitMayHaveSucceeded) {
          // A commit may already be visible. Never replay it; authoritative inventory decides.
          publish(IDLE_STATE)
          restoreTriggerFocus()
          await onCommitIndeterminate(details)
          return
        }
        if (details.recovery === 'acknowledgeWarnings') {
          publish({
            ...session,
            errorKey: getSkillOperationErrorKey(details),
            status: 'preview'
          })
          return
        }
        publish({
          context: session.inspection.context,
          details,
          errorKey: publicErrorKey(session.inspection.origin, details),
          inspection: session.inspection,
          phase: 'commit',
          previewSession: session,
          status: 'error'
        })
      }
    },
    [onCommitIndeterminate, onCommitted, publish, restoreTriggerFocus]
  )

  const commit = useCallback(async () => {
    const current = stateRef.current
    if (current.status !== 'preview') return
    await commitSession(current)
  }, [commitSession])

  const returnToPreviousStep = useCallback(() => {
    const current = stateRef.current
    if (current.status === 'idle' || current.status === 'committing') return
    const url = getFrozenUrl(current) ?? ''
    operationEpochRef.current += 1
    releaseStateAuthorities(current)
    if (getContext(current).operation === 'update') {
      publish(IDLE_STATE)
      restoreTriggerFocus()
      return
    }
    if (!url) {
      publish({
        context: { operation: 'install' },
        localError: false,
        status: 'choosingSource'
      })
      return
    }
    publish({
      context: { operation: 'install' },
      fieldError: false,
      status: 'urlInput',
      url
    })
  }, [publish, restoreTriggerFocus])

  const returnToSourceChoice = useCallback(() => {
    const current = stateRef.current
    if (current.status !== 'urlInput') return
    operationEpochRef.current += 1
    publish({
      context: current.context,
      localError: false,
      status: 'choosingSource'
    })
  }, [publish])

  const restartResolution = useCallback(
    async (current: WorkflowErrorState) => {
      const resolution = current.resolution ?? resolutionFromInspection(current.inspection)
      if (!resolution || current.context.operation !== 'install') {
        returnToPreviousStep()
        return
      }
      operationEpochRef.current += 1
      releaseStateAuthorities(current)
      await resolveUrl(current.context, {
        frozenUrl: resolution.frozenUrl,
        resolutionId: crypto.randomUUID()
      })
    },
    [resolveUrl, returnToPreviousStep]
  )

  const inspectAgain = useCallback(
    async (current: WorkflowErrorState) => {
      const inspection = current.inspection ?? current.previewSession?.inspection
      if (!inspection) {
        returnToPreviousStep()
        return
      }
      if (inspection.origin.kind === 'url') {
        await restartResolution(current)
        return
      }
      operationEpochRef.current += 1
      releaseStateAuthorities(current)
      await inspect({
        ...inspection,
        input: { ...inspection.input, preparationId: crypto.randomUUID() }
      })
    },
    [inspect, restartResolution, returnToPreviousStep]
  )

  const recover = useCallback(async () => {
    const current = stateRef.current
    if (current.status !== 'error') return
    const recovery = current.details.recovery

    if (
      (recovery === 'retrySameResolution' || recovery === 'retryLater') &&
      current.phase === 'resolve' &&
      current.resolution &&
      current.context.operation === 'install'
    ) {
      await resolveUrl(current.context, current.resolution)
      return
    }
    if (
      (recovery === 'retrySamePreparation' || recovery === 'retryLater') &&
      current.phase === 'inspect' &&
      current.inspection
    ) {
      await inspect(current.inspection)
      return
    }
    if (
      (recovery === 'retrySamePreparation' || recovery === 'retryLater') &&
      current.phase === 'commit' &&
      current.previewSession
    ) {
      await commitSession(current.previewSession)
      return
    }
    if (recovery === 'startNewResolution' || recovery === 'resolveAgain') {
      await restartResolution(current)
      return
    }
    if (recovery === 'inspectAgain' || recovery === 'newInstallationIdentity') {
      await inspectAgain(current)
      return
    }
    if (recovery === 'refreshManagement') {
      operationEpochRef.current += 1
      releaseStateAuthorities(current)
      publish(IDLE_STATE)
      restoreTriggerFocus()
      await onRefreshManagement()
      return
    }
    if (recovery === 'acknowledgeWarnings' && current.previewSession) {
      publish({ ...current.previewSession, errorKey: current.errorKey, status: 'preview' })
      return
    }
    returnToPreviousStep()
  }, [
    commitSession,
    inspect,
    inspectAgain,
    onRefreshManagement,
    publish,
    resolveUrl,
    restartResolution,
    restoreTriggerFocus,
    returnToPreviousStep
  ])

  const close = useCallback(() => {
    const current = stateRef.current
    if (current.status === 'idle' || current.status === 'committing') return
    operationEpochRef.current += 1
    releaseStateAuthorities(current)
    publish(IDLE_STATE)
    restoreTriggerFocus()
  }, [publish, restoreTriggerFocus])

  const activeUpdateSkillId = getActiveUpdateSkillId(state)

  return {
    activeUpdateSkillId,
    chooseCandidate,
    chooseGitHubSource,
    chooseLocalDirectory,
    close,
    commit,
    recover,
    returnToPreviousStep,
    returnToSourceChoice,
    startInstall,
    startUpdate,
    state,
    submitUrl,
    toggleAcknowledgement,
    updateUrl
  }
}

function getContext(state: Exclude<SkillInstallationWorkflowState, { status: 'idle' }>) {
  if (state.status === 'inspecting') return state.inspection.context
  if (state.status === 'preview' || state.status === 'committing') return state.inspection.context
  return state.context
}

function getActiveUpdateSkillId(state: SkillInstallationWorkflowState): string | null {
  if (
    state.status === 'idle' ||
    state.status === 'choosingSource' ||
    state.status === 'urlInput' ||
    state.status === 'resolving' ||
    state.status === 'candidates'
  ) {
    return null
  }
  const context = getContext(state)
  return context.operation === 'update' ? context.entry.id : null
}

function getFrozenUrl(state: SkillInstallationWorkflowState): string | undefined {
  if (state.status === 'urlInput') return state.url
  if (state.status === 'resolving' || state.status === 'candidates') {
    return state.resolution.frozenUrl
  }
  if (state.status === 'inspecting') return resolutionFromInspection(state.inspection)?.frozenUrl
  if (state.status === 'preview' || state.status === 'committing') {
    return resolutionFromInspection(state.inspection)?.frozenUrl
  }
  if (state.status === 'error') {
    return state.resolution?.frozenUrl ?? resolutionFromInspection(state.inspection)?.frozenUrl
  }
  return undefined
}

function resolutionFromInspection(
  inspection: InspectionTransaction | undefined
): ResolutionTransaction | undefined {
  if (inspection?.origin.kind !== 'url') return undefined
  return {
    frozenUrl: inspection.origin.frozenUrl,
    resolutionId: inspection.origin.resolutionId
  }
}

function getResolutionId(state: SkillInstallationWorkflowState): string | undefined {
  if (state.status === 'resolving' || state.status === 'candidates') {
    return state.resolution.resolutionId
  }
  if (state.status === 'inspecting') return resolutionFromInspection(state.inspection)?.resolutionId
  if (state.status === 'preview' || state.status === 'committing') {
    return resolutionFromInspection(state.inspection)?.resolutionId
  }
  if (state.status === 'error') {
    return (
      state.resolution?.resolutionId ??
      resolutionFromInspection(state.inspection)?.resolutionId ??
      resolutionFromInspection(state.previewSession?.inspection)?.resolutionId
    )
  }
  return undefined
}

function getPreparationId(state: SkillInstallationWorkflowState): string | undefined {
  if (state.status === 'inspecting') return state.inspection.input.preparationId
  if (state.status === 'preview' || state.status === 'committing') {
    return state.preview.preparationId
  }
  if (state.status === 'error') {
    return state.previewSession?.preview.preparationId ?? state.inspection?.input.preparationId
  }
  return undefined
}

function releaseStateAuthorities(state: SkillInstallationWorkflowState): void {
  const resolutionId = getResolutionId(state)
  const preparationId = getPreparationId(state)
  if (resolutionId) cancelResolutionBestEffort(resolutionId)
  if (preparationId) cancelPreparationBestEffort(preparationId)
}

function cancelResolutionBestEffort(resolutionId: string): void {
  void cancelSkillSourceResolution({ resolutionId }).catch(() => undefined)
}

function cancelPreparationBestEffort(preparationId: string): void {
  void cancelSkillPreparation({ preparationId }).catch(() => undefined)
}

function publicErrorKey(
  origin: InspectionOrigin,
  details: SkillOperationErrorDetails
): TranslationKey {
  return origin.kind === 'local'
    ? 'skills.localOperationFailed'
    : getSkillOperationErrorKey(details)
}
