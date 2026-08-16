import type { AgentToolResult } from '@mycopilot/protocol'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import type { Translate } from '../../../../config/translationFormat'

type ApplyPatchFailureStatus = 'failed' | 'conflict'

const APPLY_PATCH_ERROR_TRANSLATIONS: Readonly<Record<string, TranslationKey>> = {
  'agent.apply_patch.file_exists': 'agent.patch.failure.fileExists',
  'agent.apply_patch.stale_file': 'agent.patch.failure.staleFile',
  'agent.apply_patch.match_not_found': 'agent.patch.failure.matchNotFound',
  'agent.apply_patch.ambiguous_match': 'agent.patch.failure.ambiguousMatch',
  'agent.apply_patch.file_too_large': 'agent.patch.failure.tooLarge',
  'agent.apply_patch.content_too_large': 'agent.patch.failure.tooLarge',
  'agent.apply_patch.use_staged_write': 'agent.patch.failure.tooLarge',
  'agent.apply_patch.no_change': 'agent.patch.failure.noChange',
  'agent.apply_patch.not_a_file': 'agent.patch.failure.notAFile',
  'agent.apply_patch.read_failed': 'agent.patch.failure.readFailed',
  'agent.apply_patch.conflicting_input': 'agent.patch.failure.invalidRequest',
  'agent.apply_patch.invalid_create': 'agent.patch.failure.invalidRequest',
  'agent.apply_patch.missing_content': 'agent.patch.failure.invalidRequest',
  'agent.apply_patch.missing_edit': 'agent.patch.failure.invalidRequest',
  'agent.apply_patch.invalid_delete': 'agent.patch.failure.invalidRequest',
  'agent.apply_patch.invalid_edit': 'agent.patch.failure.invalidRequest'
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
