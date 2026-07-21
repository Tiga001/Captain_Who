import powerpointIcon from 'material-icon-theme/icons/powerpoint.svg?url'
import spreadsheetIcon from 'material-icon-theme/icons/table.svg?url'
import wordIcon from 'material-icon-theme/icons/word.svg?url'

export type OfficeFileKind = 'document' | 'presentation' | 'spreadsheet'

const OFFICE_FILE_ICON_BY_KIND: Readonly<Record<OfficeFileKind, string>> = {
  document: wordIcon,
  presentation: powerpointIcon,
  spreadsheet: spreadsheetIcon
}

/** Shared Office artwork used by both Skill discovery and generated-file presentation. */
export function OfficeFileIcon({ className, kind }: { className?: string; kind: OfficeFileKind }) {
  return (
    <img
      alt=""
      aria-hidden="true"
      className={className}
      draggable={false}
      src={OFFICE_FILE_ICON_BY_KIND[kind]}
    />
  )
}
