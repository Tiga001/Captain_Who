import type { TranslationKey } from '../../config/frontendTranslations'

interface NewConversationPromptKeys {
  titleKey: TranslationKey
  placeholderKey: TranslationKey
}

export const NEW_CONVERSATION_PROMPTS = [
  {
    titleKey: 'chat.newConversationPrompt.voyage.title',
    placeholderKey: 'chat.newConversationPrompt.voyage.placeholder'
  },
  {
    titleKey: 'chat.newConversationPrompt.build.title',
    placeholderKey: 'chat.newConversationPrompt.build.placeholder'
  },
  {
    titleKey: 'chat.newConversationPrompt.solve.title',
    placeholderKey: 'chat.newConversationPrompt.solve.placeholder'
  },
  {
    titleKey: 'chat.newConversationPrompt.explore.title',
    placeholderKey: 'chat.newConversationPrompt.explore.placeholder'
  },
  {
    titleKey: 'chat.newConversationPrompt.destination.title',
    placeholderKey: 'chat.newConversationPrompt.destination.placeholder'
  }
] as const satisfies readonly NewConversationPromptKeys[]

function normalizeRandomValue(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 0
  if (value >= 1) return 1 - Number.EPSILON
  return value
}

export function getRandomNewConversationPromptIndex(random: () => number = Math.random): number {
  return Math.floor(normalizeRandomValue(random()) * NEW_CONVERSATION_PROMPTS.length)
}

export function getNextRandomNewConversationPromptIndex(
  currentIndex: number,
  random: () => number = Math.random
): number {
  const promptCount = NEW_CONVERSATION_PROMPTS.length
  const normalizedCurrentIndex =
    Number.isInteger(currentIndex) && currentIndex >= 0 && currentIndex < promptCount
      ? currentIndex
      : 0
  const offset = 1 + Math.floor(normalizeRandomValue(random()) * (promptCount - 1))

  return (normalizedCurrentIndex + offset) % promptCount
}

export function getNewConversationPromptKeys(index: number): NewConversationPromptKeys {
  return NEW_CONVERSATION_PROMPTS[index] ?? NEW_CONVERSATION_PROMPTS[0]
}
