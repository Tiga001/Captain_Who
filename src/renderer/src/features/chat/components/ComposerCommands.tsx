import {
  Archive,
  FoldVertical,
  Gauge,
  Pencil,
  Pin,
  Plus,
  Split,
  type LucideIcon
} from 'lucide-react'
import { useEffect, type RefObject } from 'react'

export type ComposerCommandId = 'compact' | 'new' | 'fork' | 'usage' | 'pin' | 'rename' | 'archive'
export interface ComposerCommand {
  id: ComposerCommandId
  label: string
  description: string
  disabledReason?: string
  execute: () => void | Promise<void>
}
const icons: Record<ComposerCommandId, LucideIcon> = {
  compact: FoldVertical,
  new: Plus,
  fork: Split,
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
    <div className="composer-commands" id={listId} role="listbox">
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
