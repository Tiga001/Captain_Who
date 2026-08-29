// Renderer startup layer: defines the localized ambient phrase catalog and repeat-safe selection.

import type { AppLanguage, TranslationKey } from '../../config/frontendTranslations'

export const STARTUP_BOOTSTRAP_PHRASE_KEY = 'startup.ambient.deepThinking' as const
export const STARTUP_PHRASE_HOLD_MS = 1100
export const STARTUP_PHRASE_FADE_MS = 180
export const STARTUP_REDUCED_MOTION_HOLD_MS = 2400

const STARTUP_WORK_PHRASE_KEYS = [
  'startup.ambient.deepThinking',
  'startup.ambient.multiAgent',
  'startup.ambient.checkingCode',
  'startup.ambient.organizingContext',
  'startup.ambient.checkingHistory',
  'startup.ambient.preparingEnvironment',
  'startup.ambient.connectingCapabilities',
  'startup.ambient.buildingPlan',
  'startup.ambient.validatingResults',
  'startup.ambient.turningIdeasIntoWork',
  'startup.ambient.connectingMcpTools'
] as const satisfies readonly TranslationKey[]

const STARTUP_REFLECTIVE_PHRASE_KEYS = [
  'startup.ambient.tokenFindingPlace',
  'startup.ambient.inspirationWithinOrder',
  'startup.ambient.findingDirectionInChaos',
  'startup.ambient.shapingUnclearIdeas',
  'startup.ambient.approachingAnswers',
  'startup.ambient.contemplatingWithAi',
  'startup.ambient.possibilitiesInDetails',
  'startup.ambient.imaginationWithRigor',
  'startup.ambient.creationContinues',
  'startup.ambient.mcpOneProtocolManyTools',
  'startup.ambient.attentionIsAllYouNeed'
] as const satisfies readonly TranslationKey[]

const STARTUP_LLM_PHRASE_KEYS = [
  'startup.ambient.llmPrefillingContext',
  'startup.ambient.llmLoadingKvCache',
  'startup.ambient.llmAutoregressiveTokens',
  'startup.ambient.llmCausalMask',
  'startup.ambient.llmRotatingRope',
  'startup.ambient.llmAllocatingAttention',
  'startup.ambient.llmMultiHeadCoordination',
  'startup.ambient.llmReorderingLogits',
  'startup.ambient.llmSamplingToken',
  'startup.ambient.llmRoutingExperts',
  'startup.ambient.llmResidualStream',
  'startup.ambient.llmBackwardGradients',
  'startup.ambient.llmSgd',
  'startup.ambient.llmLossLandscape',
  'startup.ambient.llmSupervisedFineTuning',
  'startup.ambient.llmPreferenceScoring',
  'startup.ambient.llmRewardBackprop',
  'startup.ambient.llmReinforcementAlignment',
  'startup.ambient.llmLora',
  'startup.ambient.llmQuantization',
  'startup.ambient.llmTensorParallel',
  'startup.ambient.llmPrefixCache',
  'startup.ambient.llmContextCompression',
  'startup.ambient.llmUncertaintyCalibration',
  'startup.ambient.llmAttentionCost',
  'startup.ambient.llmContextMemory',
  'startup.ambient.llmLossCaveat',
  'startup.ambient.llmExpertAntiAverage',
  'startup.ambient.llmTemperature',
  'startup.ambient.llmEntropy',
  'startup.ambient.llmParametersAnswer',
  'startup.ambient.llmEmergenceScale',
  'startup.ambient.llmGradientWayHome',
  'startup.ambient.llmAttentionStillAll',
  'startup.ambient.llmMcpServerHandshake',
  'startup.ambient.llmJsonRpcIntent'
] as const satisfies readonly TranslationKey[]

export const STARTUP_AMBIENT_PHRASE_GROUPS = [
  STARTUP_WORK_PHRASE_KEYS,
  STARTUP_REFLECTIVE_PHRASE_KEYS,
  STARTUP_LLM_PHRASE_KEYS
] as const satisfies readonly (readonly TranslationKey[])[]

const STARTUP_CHARACTER_INTERVAL_MS = {
  'zh-CN': 74,
  'zh-TW': 74,
  'en-US': 52,
  'en-GB': 52,
  'ko-KR': 74,
  'ja-JP': 74,
  'fr-FR': 52,
  'it-IT': 52,
  'ru-RU': 52
} as const satisfies Record<AppLanguage, number>

export function getStartupCharacterIntervalMs(language: AppLanguage): number {
  return STARTUP_CHARACTER_INTERVAL_MS[language]
}

export function pickStartupPhraseGroupIndex(
  groupCount: number,
  random: () => number = Math.random
): number {
  if (groupCount <= 1) return 0
  return Math.min(groupCount - 1, Math.floor(random() * groupCount))
}

export function pickNextStartupPhraseIndex(
  currentIndex: number,
  phraseCount: number,
  random: () => number = Math.random
): number {
  if (phraseCount <= 1) return 0

  // Select from every slot except the current one, so the loop never repeats immediately.
  const randomSlot = Math.min(phraseCount - 2, Math.floor(random() * (phraseCount - 1)))
  return randomSlot >= currentIndex ? randomSlot + 1 : randomSlot
}

export function pickFirstStartupPhraseIndex(
  phraseKeys: readonly TranslationKey[],
  excludedKey: TranslationKey,
  random: () => number = Math.random
): number {
  if (phraseKeys.length <= 1) return 0

  const excludedIndex = phraseKeys.indexOf(excludedKey)
  if (excludedIndex >= 0) {
    return pickNextStartupPhraseIndex(excludedIndex, phraseKeys.length, random)
  }

  return Math.min(phraseKeys.length - 1, Math.floor(random() * phraseKeys.length))
}
