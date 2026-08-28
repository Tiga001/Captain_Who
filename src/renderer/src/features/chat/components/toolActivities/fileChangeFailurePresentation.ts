import type { AgentToolResult } from '@mycopilot/protocol'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import type { Translate } from '../../../../config/translationFormat'

type FileChangeFailureStatus = 'failed' | 'conflict' | 'outcome_unknown'

const FILE_CHANGE_ERROR_TRANSLATIONS: Readonly<Record<string, TranslationKey>> = {
  'agent.apply_patch.file_exists': 'agent.fileChange.failure.fileExists',
  'agent.apply_patch.file_missing': 'agent.fileChange.failure.staleFile',
  'agent.apply_patch.revision_conflict': 'agent.fileChange.failure.staleFile',
  'agent.apply_patch.observation_required': 'agent.fileChange.failure.staleFile',
  'agent.apply_patch.observation_expired': 'agent.fileChange.failure.staleFile',
  'agent.apply_patch.observation_owner_mismatch': 'agent.fileChange.failure.staleFile',
  'agent.apply_patch.observation_path_mismatch': 'agent.fileChange.failure.staleFile',
  'agent.apply_patch.observation_stale': 'agent.fileChange.failure.staleFile',
  'agent.apply_patch.match_not_found': 'agent.fileChange.failure.matchNotFound',
  'agent.apply_patch.ambiguous_match': 'agent.fileChange.failure.ambiguousMatch',
  'agent.apply_patch.content_too_large': 'agent.fileChange.failure.tooLarge',
  'agent.apply_patch.no_change': 'agent.fileChange.failure.noChange',
  'agent.apply_patch.not_regular_file': 'agent.fileChange.failure.notAFile',
  'agent.apply_patch.invalid_arguments': 'agent.fileChange.failure.invalidRequest',
  'agent.apply_patch.unknown_field': 'agent.fileChange.failure.invalidRequest',
  'agent.apply_patch.illegal_field_combination': 'agent.fileChange.failure.invalidRequest',
  'agent.apply_patch.conflict': 'agent.fileChange.failure.conflict',
  'agent.apply_patch.outcome_unknown': 'agent.fileChange.failure.outcomeUnknown'
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}

function getStructuredErrorCode(result: AgentToolResult | undefined): string | undefined {
  if (!isRecord(result?.result)) return undefined
  const errorCode = result.result.errorCode
  return typeof errorCode === 'string' && errorCode.length <= 128 ? errorCode : undefined
}

/** Maps typed Host failures to the bounded, localized FileChange presentation contract. */
export function getSafeFileChangeFailureMessage(
  status: FileChangeFailureStatus,
  result: AgentToolResult | undefined,
  t: Translate
): string {
  const errorCode = getStructuredErrorCode(result)
  const translationKey = errorCode ? FILE_CHANGE_ERROR_TRANSLATIONS[errorCode] : undefined
  if (translationKey) return t(translationKey)
  if (status === 'outcome_unknown') return t('agent.fileChange.failure.outcomeUnknown')
  return t(
    status === 'conflict' ? 'agent.fileChange.failure.conflict' : 'agent.fileChange.failure.generic'
  )
}
