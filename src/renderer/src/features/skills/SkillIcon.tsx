// Renderer Skill icon presentation: maps exact bundled application identities to stable icons.
import type { SkillSourceDescriptor } from '@mycopilot/protocol'
import { WandSparkles } from 'lucide-react'
import pdfIcon from 'material-icon-theme/icons/pdf.svg?url'
import imageGenerationIcon from '../../assets/skill-icons/fluent-artist-palette-flat.svg?url'
import skillCreatorIcon from '../../assets/skill-icons/fluent-magic-wand-flat.svg?url'
import skillInstallerIcon from '../../assets/skill-icons/fluent-toolbox-flat.svg?url'
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
  Partial<Record<BundledSkillPresentationKey, OfficeFileKind>>
> = {
  documents: 'document',
  presentations: 'presentation',
  spreadsheets: 'spreadsheet'
}

const COLORED_SKILL_ICON_BY_PRESENTATION_KEY: Readonly<
  Partial<Record<BundledSkillPresentationKey, string>>
> = {
  imageGeneration: imageGenerationIcon,
  skillCreator: skillCreatorIcon,
  skillInstaller: skillInstallerIcon
}

function getOfficeSkillKind(skillId: string, source?: SkillSourceDescriptor) {
  const presentationKey = getBundledSkillPresentationKey(skillId, source)
  return presentationKey ? OFFICE_SKILL_KIND_BY_PRESENTATION_KEY[presentationKey] : undefined
}

export function SkillIcon({ className, skillId, source }: SkillIconProps) {
  // Exact protocol identities keep icon selection stable across localization and renamed displays.
  const presentationKey = getBundledSkillPresentationKey(skillId, source)
  const officeKind = getOfficeSkillKind(skillId, source)
  const coloredIcon = presentationKey
    ? COLORED_SKILL_ICON_BY_PRESENTATION_KEY[presentationKey]
    : undefined

  return (
    <span className={className} data-office-kind={officeKind} aria-hidden="true">
      {officeKind ? (
        <OfficeFileIcon kind={officeKind} />
      ) : presentationKey === 'pdf' ? (
        <img alt="" draggable={false} src={pdfIcon} />
      ) : coloredIcon ? (
        <img alt="" draggable={false} src={coloredIcon} />
      ) : (
        <WandSparkles />
      )}
    </span>
  )
}
