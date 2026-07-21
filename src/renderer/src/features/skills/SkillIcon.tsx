// Renderer Skill icon presentation: maps exact bundled Office identities to bundled MIT icons.
import type { SkillSourceDescriptor } from '@mycopilot/protocol'
import { WandSparkles } from 'lucide-react'
import powerpointIcon from 'material-icon-theme/icons/powerpoint.svg?url'
import spreadsheetIcon from 'material-icon-theme/icons/table.svg?url'
import wordIcon from 'material-icon-theme/icons/word.svg?url'
import {
  getBundledSkillPresentationKey,
  type BundledSkillPresentationKey
} from './skillPresentation'

type OfficeSkillKind = 'document' | 'presentation' | 'spreadsheet'

interface SkillIconProps {
  className?: string
  skillId: string
  source?: SkillSourceDescriptor
}

const OFFICE_SKILL_KIND_BY_PRESENTATION_KEY: Readonly<
  Record<BundledSkillPresentationKey, OfficeSkillKind | undefined>
> = {
  documents: 'document',
  presentations: 'presentation',
  repositoryEvidenceAuditor: undefined,
  spreadsheets: 'spreadsheet'
}

function getOfficeSkillKind(skillId: string, source?: SkillSourceDescriptor) {
  const presentationKey = getBundledSkillPresentationKey(skillId, source)
  return presentationKey ? OFFICE_SKILL_KIND_BY_PRESENTATION_KEY[presentationKey] : undefined
}

const OFFICE_SKILL_ICON_BY_KIND: Readonly<Record<OfficeSkillKind, string>> = {
  document: wordIcon,
  presentation: powerpointIcon,
  spreadsheet: spreadsheetIcon
}

export function SkillIcon({ className, skillId, source }: SkillIconProps) {
  // Exact protocol identities keep icon selection stable across localization and renamed displays.
  const officeKind = getOfficeSkillKind(skillId, source)

  return (
    <span className={className} data-office-kind={officeKind} aria-hidden="true">
      {officeKind ? (
        <img alt="" draggable={false} src={OFFICE_SKILL_ICON_BY_KIND[officeKind]} />
      ) : (
        <WandSparkles />
      )}
    </span>
  )
}
