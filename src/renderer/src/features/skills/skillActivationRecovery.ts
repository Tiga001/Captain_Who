import { HostInvocationError } from '@mycopilot/host-api'
import type {
  SkillActivationErrorCode,
  SkillActivationErrorData,
  SkillRecovery,
  SkillSelection
} from '@mycopilot/protocol'
import { mergeSkillSelections, normalizeSkillSelections } from './skillSelection'

export type SkillActivationRecoveryPlan =
  | {
      draftPolicy: 'restoreMissing'
      refreshCatalog: boolean
      selectionsToRestore: SkillSelection[]
    }
  | {
      draftPolicy: 'rejectSelection'
      refreshCatalog: false
      rejectedSkillId: string
      selectionsToRestore: SkillSelection[]
    }
  | {
      draftPolicy: 'discardSubmitted'
      refreshCatalog: false
      selectionsToRestore: SkillSelection[]
    }

const SKILL_ACTIVATION_ERROR_CODES = new Set<SkillActivationErrorCode>([
  'invalidSelection',
  'duplicateSelection',
  'tooManySkills',
  'activationTooLarge',
  'notFound',
  'stale',
  'invalidSkill',
  'sourceUnavailable'
])

const SKILL_RECOVERY_POLICIES = new Set<SkillRecovery>([
  'retrySameSelection',
  'refreshCatalog',
  'rejectSelection'
])

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}

function isOptionalString(value: unknown): value is string | undefined {
  return value === undefined || typeof value === 'string'
}

function isSkillActivationErrorData(value: unknown): value is SkillActivationErrorData {
  if (!isRecord(value)) return false

  return (
    value.type === 'skillActivation' &&
    typeof value.code === 'string' &&
    SKILL_ACTIVATION_ERROR_CODES.has(value.code as SkillActivationErrorCode) &&
    typeof value.recovery === 'string' &&
    SKILL_RECOVERY_POLICIES.has(value.recovery as SkillRecovery) &&
    typeof value.message === 'string' &&
    isOptionalString(value.skillId) &&
    isOptionalString(value.expectedRevision) &&
    isOptionalString(value.actualRevision)
  )
}

/**
 * Converts the host's typed activation failure into draft recovery behavior.
 *
 * A malformed payload that claims to be a Skill activation failure is rejected conservatively:
 * no selection is restored and no recovery action is inferred from unvalidated data. Ordinary
 * non-Skill failures remain retryable so a transient transport failure does not discard an
 * explicit user selection.
 */
export function planSkillActivationRecovery(
  error: unknown,
  submittedSelections: readonly SkillSelection[]
): SkillActivationRecoveryPlan {
  const selections = normalizeSkillSelections(submittedSelections)
  if (!(error instanceof HostInvocationError)) {
    return {
      draftPolicy: 'restoreMissing',
      refreshCatalog: false,
      selectionsToRestore: selections
    }
  }

  const data = error.data
  if (!isSkillActivationErrorData(data)) {
    const claimsSkillActivation = isRecord(data) && data.type === 'skillActivation'
    return {
      draftPolicy: claimsSkillActivation ? 'discardSubmitted' : 'restoreMissing',
      refreshCatalog: false,
      selectionsToRestore: claimsSkillActivation ? [] : selections
    }
  }

  switch (data.recovery) {
    case 'retrySameSelection':
      return {
        draftPolicy: 'restoreMissing',
        refreshCatalog: false,
        selectionsToRestore: selections
      }
    case 'refreshCatalog':
      return {
        draftPolicy: 'restoreMissing',
        refreshCatalog: true,
        selectionsToRestore: selections
      }
    case 'rejectSelection': {
      if (!data.skillId || !selections.some((selection) => selection.id === data.skillId)) {
        return {
          draftPolicy: 'discardSubmitted',
          refreshCatalog: false,
          selectionsToRestore: []
        }
      }
      return {
        draftPolicy: 'rejectSelection',
        refreshCatalog: false,
        rejectedSkillId: data.skillId,
        selectionsToRestore: selections.filter((selection) => selection.id !== data.skillId)
      }
    }
  }
}

/**
 * Reconciles an activation failure with edits made after the failed request was submitted.
 *
 * The live draft is authoritative for every id it already contains. Submitted selections only
 * fill missing ids, while an explicitly rejected id is removed even when it was re-selected during
 * the request. This prevents a late failure from reverting a newer user-selected revision.
 */
export function reconcileSkillActivationSelections(
  currentSelections: readonly SkillSelection[],
  plan: SkillActivationRecoveryPlan
): SkillSelection[] {
  const current = normalizeSkillSelections(currentSelections)
  const retainedCurrent =
    plan.draftPolicy === 'rejectSelection'
      ? current.filter((selection) => selection.id !== plan.rejectedSkillId)
      : current

  if (plan.draftPolicy === 'discardSubmitted') return retainedCurrent
  return mergeSkillSelections(retainedCurrent, plan.selectionsToRestore)
}
