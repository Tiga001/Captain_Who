// Renderer Skill presentation: localizes only exact application-bundled Skill identities.
import type { SkillSourceDescriptor } from '@mycopilot/protocol'
import type { Translate } from '../../config/translationFormat'
import type { TranslationKey } from '../../config/frontendTranslations'

export type BundledSkillPresentationKey =
  | 'documents'
  | 'imageGeneration'
  | 'pdf'
  | 'presentations'
  | 'skillCreator'
  | 'skillInstaller'
  | 'spreadsheets'

interface SkillPresentationInput {
  description?: string
  id: string
  name: string
  source?: SkillSourceDescriptor
}

interface SkillPresentation {
  description: string
  name: string
}

const BUNDLED_SKILL_KEY_BY_ID: Readonly<Record<string, BundledSkillPresentationKey>> = {
  'bundled:application:documents': 'documents',
  'bundled:application:image-generation': 'imageGeneration',
  'bundled:application:pdf': 'pdf',
  'bundled:application:presentations': 'presentations',
  'bundled:application:skill-creator': 'skillCreator',
  'bundled:application:skill-installer': 'skillInstaller',
  'bundled:application:spreadsheets': 'spreadsheets'
}

const BUNDLED_SKILL_KEY_BY_SOURCE_ID: Readonly<Record<string, BundledSkillPresentationKey>> = {
  'application:documents': 'documents',
  'application:image-generation': 'imageGeneration',
  'application:pdf': 'pdf',
  'application:presentations': 'presentations',
  'application:skill-creator': 'skillCreator',
  'application:skill-installer': 'skillInstaller',
  'application:spreadsheets': 'spreadsheets'
}

const BUNDLED_SKILL_DISPLAY_ORDER: Readonly<Record<BundledSkillPresentationKey, number>> = {
  skillInstaller: 0,
  skillCreator: 1,
  imageGeneration: 2,
  documents: 3,
  presentations: 4,
  spreadsheets: 5,
  pdf: 6
}

const BUNDLED_SKILL_TRANSLATIONS: Readonly<
  Record<BundledSkillPresentationKey, { description: TranslationKey; name: TranslationKey }>
> = {
  documents: {
    description: 'skills.bundled.documents.description',
    name: 'skills.bundled.documents.name'
  },
  imageGeneration: {
    description: 'skills.bundled.imageGeneration.description',
    name: 'skills.bundled.imageGeneration.name'
  },
  pdf: {
    description: 'skills.bundled.pdf.description',
    name: 'skills.bundled.pdf.name'
  },
  presentations: {
    description: 'skills.bundled.presentations.description',
    name: 'skills.bundled.presentations.name'
  },
  skillInstaller: {
    description: 'skills.bundled.skillInstaller.description',
    name: 'skills.bundled.skillInstaller.name'
  },
  skillCreator: {
    description: 'skills.bundled.skillCreator.description',
    name: 'skills.bundled.skillCreator.name'
  },
  spreadsheets: {
    description: 'skills.bundled.spreadsheets.description',
    name: 'skills.bundled.spreadsheets.name'
  }
}

export function getBundledSkillPresentationKey(
  skillId: string,
  source?: SkillSourceDescriptor
): BundledSkillPresentationKey | undefined {
  return (
    BUNDLED_SKILL_KEY_BY_ID[skillId] ??
    (source?.kind === 'bundled' ? BUNDLED_SKILL_KEY_BY_SOURCE_ID[source.id] : undefined)
  )
}

export function sortSkillsForDisplay<T extends SkillPresentationInput>(skills: readonly T[]): T[] {
  return skills
    .map((skill, index) => ({ index, rank: bundledSkillDisplayRank(skill), skill }))
    .sort((left, right) => {
      if (left.rank !== right.rank) return left.rank - right.rank
      return left.index - right.index
    })
    .map((entry) => entry.skill)
}

function bundledSkillDisplayRank(skill: SkillPresentationInput): number {
  const key = getBundledSkillPresentationKey(skill.id, skill.source)
  return key === undefined ? Number.POSITIVE_INFINITY : BUNDLED_SKILL_DISPLAY_ORDER[key]
}

export function getSkillPresentation(
  skill: SkillPresentationInput,
  t: Translate
): SkillPresentation {
  const bundledKey = getBundledSkillPresentationKey(skill.id, skill.source)
  if (!bundledKey) {
    return { description: skill.description ?? '', name: skill.name }
  }

  const translation = BUNDLED_SKILL_TRANSLATIONS[bundledKey]
  return {
    description: t(translation.description),
    name: t(translation.name)
  }
}
