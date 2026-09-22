import {
  Archive,
  Blocks,
  Bot,
  Box,
  Check,
  FileDiff,
  FoldVertical,
  Gauge,
  Globe2,
  Pencil,
  Paperclip,
  Folder,
  ImageIcon,
  Pin,
  Plus,
  Split,
  Sparkles,
  SunMoon,
  TerminalSquare,
  type LucideIcon
} from 'lucide-react'
import { useEffect, useId, type ReactNode, type RefObject } from 'react'

export type ComposerCommandId =
  | 'model'
  | 'theme'
  | 'compact'
  | 'new'
  | 'fork'
  | 'capabilities'
  | 'terminal'
  | 'browser'
  | 'review'
  | 'agents'
  | 'usage'
  | 'pin'
  | 'rename'
  | 'archive'
interface ComposerCommandPresentation {
  label: string
  description: string
  disabledReason?: string
}
export type ComposerCommand = ComposerCommandPresentation &
  (
    | { id: 'model' }
    | { id: 'capabilities' }
    | {
        id: Exclude<ComposerCommandId, 'model' | 'capabilities'>
        execute: () => void | Promise<void>
      }
  )

export interface ComposerAddMenuProps {
  addFileLabel: string
  addFolderLabel: string
  addImageLabel: string
  addMenuTitle: string
  onAddFile: () => void | Promise<void>
  onAddFolder: () => void | Promise<void>
  onAddImage: () => void | Promise<void>
  onSelectIndex?: (index: number) => void
  selectedIndex?: number
  skillEmptyLabel?: string
  skillLoadingLabel?: string
  skillErrorLabel?: string
  skillRetryLabel?: string
  skillTruncatedLabel?: string
  skillDiagnosticsLabel?: string
  skillDiagnosticsAvailableLabel?: string
  skillDiagnosticsCount?: number
  skillCatalogTruncated?: boolean
  skillError?: boolean
  skillLoading?: boolean
  skills?: readonly ComposerAddMenuSkill[]
  skillsTitle?: string
  onRetrySkills?: () => void
  onToggleSkill?: (skill: ComposerAddMenuSkill) => void
}

export interface ComposerAddMenuSkill {
  id: string
  label: string
  accessibleLabel?: string
  description?: string
  icon: ReactNode
  selected?: boolean
  disabled?: boolean
}

/** The plus and @ entry points intentionally share the slash menu's row treatment. */
export function ComposerAddMenu({
  addFileLabel,
  addFolderLabel,
  addImageLabel,
  addMenuTitle,
  onAddFile,
  onAddFolder,
  onAddImage,
  onSelectIndex,
  selectedIndex = -1,
  skillEmptyLabel,
  skillLoading = false,
  skillLoadingLabel,
  skillError = false,
  skillErrorLabel,
  skillRetryLabel,
  skillTruncatedLabel,
  skillDiagnosticsLabel,
  skillDiagnosticsAvailableLabel,
  skillDiagnosticsCount = 0,
  skillCatalogTruncated = false,
  skills,
  skillsTitle,
  onRetrySkills,
  onToggleSkill
}: ComposerAddMenuProps) {
  const listId = useId()
  const items = [
    { id: 'file', label: addFileLabel, icon: <Paperclip aria-hidden="true" />, onClick: onAddFile },
    {
      id: 'folder',
      label: addFolderLabel,
      icon: <Folder aria-hidden="true" />,
      onClick: onAddFolder
    },
    {
      id: 'image',
      label: addImageLabel,
      icon: <ImageIcon aria-hidden="true" />,
      onClick: onAddImage
    },
    ...(skills ?? []).map((skill) => ({
      ...skill,
      id: `skill:${skill.id}`,
      onClick: () => onToggleSkill?.(skill)
    }))
  ]

  useEffect(() => {
    if (selectedIndex < 0) return
    const selected = document.getElementById(`${listId}-${selectedIndex}`)
    selected?.scrollIntoView({ block: 'nearest' })
  }, [listId, selectedIndex])

  return (
    <div className="composer-commands composer-add-menu" role="menu" aria-label={addMenuTitle}>
      <div className="composer-commands__list" id={listId}>
        <p className="composer-add-menu__title">{addMenuTitle}</p>
        {items.slice(0, 3).map((item, index) => (
          <button
            aria-selected={index === selectedIndex}
            id={`${listId}-${index}`}
            key={item.id}
            role="menuitem"
            type="button"
            onFocus={() => onSelectIndex?.(index)}
            onPointerMove={() => onSelectIndex?.(index)}
            onClick={() => void item.onClick()}
          >
            {item.icon}
            <span className="composer-commands__label">{item.label}</span>
          </button>
        ))}

        {skills && (
          <>
            <p className="composer-add-menu__title composer-add-menu__section-title">
              {skillsTitle}
            </p>
            {skillCatalogTruncated && skillTruncatedLabel && (
              <p className="composer-commands__notice" role="status">
                {skillTruncatedLabel}
              </p>
            )}
            {skillDiagnosticsCount > 0 && skillDiagnosticsLabel && (
              <details className="composer-commands__diagnostics">
                <summary>
                  {skillDiagnosticsLabel.replace('{count}', String(skillDiagnosticsCount))}
                </summary>
                {skillDiagnosticsAvailableLabel && <p>{skillDiagnosticsAvailableLabel}</p>}
              </details>
            )}
            {skillLoading ? (
              <p className="composer-commands__state" role="status">
                <Sparkles aria-hidden="true" />
                {skillLoadingLabel}
              </p>
            ) : skillError ? (
              <div className="composer-commands__state" role="alert">
                <Sparkles aria-hidden="true" />
                <span>{skillErrorLabel}</span>
                {onRetrySkills && skillRetryLabel && (
                  <button type="button" onClick={onRetrySkills}>
                    {skillRetryLabel}
                  </button>
                )}
              </div>
            ) : skills.length === 0 ? (
              <p className="composer-commands__empty">{skillEmptyLabel}</p>
            ) : (
              skills.map((skill, skillIndex) => {
                const index = skillIndex + 3
                return (
                  <button
                    aria-label={skill.accessibleLabel}
                    aria-disabled={skill.disabled || undefined}
                    aria-selected={index === selectedIndex}
                    data-disabled={skill.disabled || undefined}
                    data-selected={skill.selected || undefined}
                    id={`${listId}-${index}`}
                    key={skill.id}
                    role="menuitem"
                    title={skill.description}
                    type="button"
                    onFocus={() => onSelectIndex?.(index)}
                    onPointerMove={() => onSelectIndex?.(index)}
                    onClick={() => {
                      if (!skill.disabled || skill.selected) onToggleSkill?.(skill)
                    }}
                  >
                    <span className="composer-add-menu__skill-icon">{skill.icon}</span>
                    <span className="composer-commands__label">{skill.label}</span>
                    {skill.description && (
                      <span className="composer-commands__description" title={skill.description}>
                        {skill.description}
                      </span>
                    )}
                    {skill.selected && (
                      <Check
                        aria-label="selected"
                        className="composer-add-menu__selected-icon"
                        aria-hidden="true"
                      />
                    )}
                  </button>
                )
              })
            )}
          </>
        )}
      </div>
    </div>
  )
}
const icons: Record<ComposerCommandId, LucideIcon> = {
  model: Box,
  theme: SunMoon,
  compact: FoldVertical,
  new: Plus,
  fork: Split,
  capabilities: Blocks,
  terminal: TerminalSquare,
  browser: Globe2,
  review: FileDiff,
  agents: Bot,
  usage: Gauge,
  pin: Pin,
  rename: Pencil,
  archive: Archive
}

export function filterComposerCommands(
  commands: readonly ComposerCommand[],
  message: string
): ComposerCommand[] {
  const query = message.slice(1).trim().toLocaleLowerCase()
  return commands.filter((command) =>
    `${command.id} ${command.label}`.toLocaleLowerCase().includes(query)
  )
}

export function ComposerCommands({
  commands,
  query,
  selectedIndex,
  onSelect,
  onExecute,
  emptyLabel,
  listId,
  scrollContainerRef
}: {
  commands: readonly ComposerCommand[]
  query: string
  selectedIndex: number
  onSelect: (index: number) => void
  onExecute: (command: ComposerCommand) => void
  emptyLabel: string
  listId: string
  scrollContainerRef?: RefObject<HTMLDivElement | null>
}) {
  useEffect(() => {
    const selected = document.getElementById(`${listId}-${selectedIndex}`)
    if (!selected) return
    const boundary = scrollContainerRef?.current
    if (!boundary) {
      selected.scrollIntoView({ block: 'nearest' })
      return
    }
    // Reveal the selected command inside the menu and its portal viewport without letting
    // scrollIntoView move the new-conversation page or the application shell.
    for (let container = selected.parentElement; container; container = container.parentElement) {
      const selectedRect = selected.getBoundingClientRect()
      const containerRect = container.getBoundingClientRect()
      if (selectedRect.top < containerRect.top) {
        container.scrollTop += selectedRect.top - containerRect.top
      } else if (selectedRect.bottom > containerRect.bottom) {
        container.scrollTop += selectedRect.bottom - containerRect.bottom
      }
      if (container === boundary) break
    }
  }, [listId, scrollContainerRef, selectedIndex])
  return (
    <div className="composer-commands">
      <div className="composer-commands__list" id={listId} role="listbox">
        {commands.length === 0 ? (
          <p className="composer-commands__empty">{emptyLabel}</p>
        ) : (
          commands.map((command, index) => {
            const Icon = icons[command.id]
            return (
              <button
                id={`${listId}-${index}`}
                key={command.id}
                type="button"
                role="option"
                aria-selected={index === selectedIndex}
                aria-disabled={Boolean(command.disabledReason)}
                data-disabled={Boolean(command.disabledReason)}
                onPointerMove={() => onSelect(index)}
                onPointerEnter={() => onSelect(index)}
                onFocus={() => onSelect(index)}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => onExecute(command)}
              >
                <Icon aria-hidden="true" />
                <span className="composer-commands__label">
                  <CommandMatch label={command.label} query={query} />
                </span>
                <span className="composer-commands__description">
                  {command.disabledReason ?? command.description}
                </span>
              </button>
            )
          })
        )}
      </div>
    </div>
  )
}

function CommandMatch({ label, query }: { label: string; query: string }) {
  const normalized = query.trim().toLocaleLowerCase()
  const index = normalized ? label.toLocaleLowerCase().indexOf(normalized) : -1
  if (index < 0) return label
  return (
    <>
      {label.slice(0, index)}
      <strong>{label.slice(index, index + normalized.length)}</strong>
      {label.slice(index + normalized.length)}
    </>
  )
}
