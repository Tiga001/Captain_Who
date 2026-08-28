import type { AgentToolResult } from '@mycopilot/protocol'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import type { Translate } from '../../../../config/translationFormat'

type ApplyPatchFailureStatus = 'failed' | 'conflict'

const APPLY_PATCH_ERROR_TRANSLATIONS: Readonly<Record<string, TranslationKey>> = {
  'agent.apply_patch.file_exists': 'agent.patch.failure.fileExists',
  'agent.apply_patch.file_missing': 'agent.patch.failure.staleFile',
  'agent.apply_patch.revision_conflict': 'agent.patch.failure.staleFile',
  'agent.apply_patch.observation_required': 'agent.patch.failure.staleFile',
  'agent.apply_patch.observation_expired': 'agent.patch.failure.staleFile',
  'agent.apply_patch.observation_owner_mismatch': 'agent.patch.failure.staleFile',
  'agent.apply_patch.observation_path_mismatch': 'agent.patch.failure.staleFile',
  'agent.apply_patch.observation_stale': 'agent.patch.failure.staleFile',
  'agent.apply_patch.match_not_found': 'agent.patch.failure.matchNotFound',
  'agent.apply_patch.ambiguous_match': 'agent.patch.failure.ambiguousMatch',
  'agent.apply_patch.content_too_large': 'agent.patch.failure.tooLarge',
  'agent.apply_patch.no_change': 'agent.patch.failure.noChange',
  'agent.apply_patch.not_regular_file': 'agent.patch.failure.notAFile',
  'agent.apply_patch.invalid_arguments': 'agent.patch.failure.invalidRequest',
  'agent.apply_patch.unknown_field': 'agent.patch.failure.invalidRequest',
  'agent.apply_patch.illegal_field_combination': 'agent.patch.failure.invalidRequest',
  'agent.apply_patch.conflict': 'agent.patch.failure.conflict',
  'agent.apply_patch.outcome_unknown': 'agent.patch.failure.outcomeUnknown'
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}

function getStructuredErrorCode(result: AgentToolResult | undefined): string | undefined {
  if (!isRecord(result?.result)) return undefined
  const errorCode = result.result.errorCode
  return typeof errorCode === 'string' && errorCode.length <= 128 ? errorCode : undefined
}

/** Projects backend apply_patch failures onto a small, localized display contract. */
export function getSafeApplyPatchFailureMessage(
  status: ApplyPatchFailureStatus,
  result: AgentToolResult | undefined,
  t: Translate
): string {
  const errorCode = getStructuredErrorCode(result)
  const translationKey = errorCode ? APPLY_PATCH_ERROR_TRANSLATIONS[errorCode] : undefined
  if (translationKey) return t(translationKey)
  return t(status === 'conflict' ? 'agent.patch.failure.conflict' : 'agent.patch.failure.generic')
}
