// Renderer Skill icon presentation: maps exact bundled Office identities to bundled MIT icons.
import type { SkillSourceDescriptor } from '@mycopilot/protocol'
import { WandSparkles } from 'lucide-react'
import { OfficeFileIcon, type OfficeFileKind } from '../../components/files/OfficeFileIcon'
import {
  getBundledSkillPresentationKey,
  type BundledSkillPresentationKey
} from './skillPresentation'

interface SkillIconProps {
  className?: string
  skillId: string
  source?: SkillSourceDescriptor
}

const OFFICE_SKILL_KIND_BY_PRESENTATION_KEY: Readonly<
  Record<BundledSkillPresentationKey, OfficeFileKind | undefined>
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

export function SkillIcon({ className, skillId, source }: SkillIconProps) {
  // Exact protocol identities keep icon selection stable across localization and renamed displays.
  const officeKind = getOfficeSkillKind(skillId, source)

  return (
    <span className={className} data-office-kind={officeKind} aria-hidden="true">
      {officeKind ? <OfficeFileIcon kind={officeKind} /> : <WandSparkles />}
    </span>
  )
}
