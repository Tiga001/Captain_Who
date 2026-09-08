import { defineSettingsNodes } from '../settingsDefinition'

export const WORK_MODE_OPTIONS = [
  {
    value: 'coding',
    titleKey: 'personalization.workModeCoding',
    descriptionKey: 'personalization.workModeCodingDescription'
  },
  {
    value: 'general',
    titleKey: 'personalization.workModeGeneral',
    descriptionKey: 'personalization.workModeGeneralDescription'
  }
] as const

export const TONE_OPTIONS = [
  {
    value: 'friendly',
    titleKey: 'personalization.toneFriendly',
    descriptionKey: 'personalization.toneFriendlyDescription'
  },
  {
    value: 'pragmatic',
    titleKey: 'personalization.tonePragmatic',
    descriptionKey: 'personalization.tonePragmaticDescription'
  }
] as const

export const humanInteractionSettingsNodes = defineSettingsNodes([
  {
    id: 'personalization.humanInteraction',
    title: 'humanInteraction.settings.title',
    children: [
      { id: 'personalization.allowQuestions', title: 'humanInteraction.settings.allowQuestions' }
    ]
  }
])

export const personalizationSettingsNodes = defineSettingsNodes([
  {
    id: 'personalization.workMode',
    title: 'personalization.workMode',
    description: 'personalization.workModeDescription',
    terms: WORK_MODE_OPTIONS.flatMap((option) => [option.titleKey, option.descriptionKey]),
    children: [
      {
        id: 'personalization.minimalMode',
        title: 'personalization.minimalMode',
        description: 'personalization.minimalModeDescription'
      }
    ]
  },
  {
    id: 'personalization.tone',
    title: 'personalization.tone',
    description: 'personalization.toneDescription',
    terms: TONE_OPTIONS.flatMap((option) => [option.titleKey, option.descriptionKey])
  },
  ...humanInteractionSettingsNodes,
  {
    id: 'personalization.customInstructions',
    title: 'personalization.customInstructions',
    description: 'personalization.customInstructionsDescription'
  }
])
