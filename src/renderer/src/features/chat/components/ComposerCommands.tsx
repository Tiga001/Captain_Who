import {
  Archive,
  Blocks,
  Bot,
  Box,
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
import { useEffect, type RefObject } from 'react'

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
  onAddSkill?: () => void | Promise<void>
  skillLabel: string
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
  onAddSkill,
  skillLabel
}: ComposerAddMenuProps) {
  return (
    <div className="composer-commands composer-add-menu" role="menu" aria-label={addMenuTitle}>
      <div className="composer-commands__list">
        <p className="composer-add-menu__title">{addMenuTitle}</p>
        <button type="button" role="menuitem" onClick={() => void onAddFile()}>
          <Paperclip aria-hidden="true" />
          <span className="composer-commands__label">{addFileLabel}</span>
        </button>
        <button type="button" role="menuitem" onClick={() => void onAddFolder()}>
          <Folder aria-hidden="true" />
          <span className="composer-commands__label">{addFolderLabel}</span>
        </button>
        <button type="button" role="menuitem" onClick={() => void onAddImage()}>
          <ImageIcon aria-hidden="true" />
          <span className="composer-commands__label">{addImageLabel}</span>
        </button>
        {onAddSkill && (
          <button type="button" role="menuitem" onClick={() => void onAddSkill()}>
            <Sparkles aria-hidden="true" />
            <span className="composer-commands__label">{skillLabel}</span>
          </button>
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
