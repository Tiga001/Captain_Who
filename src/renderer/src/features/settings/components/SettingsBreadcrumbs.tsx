import { ChevronRight } from 'lucide-react'

export interface SettingsBreadcrumbItem {
  id: string
  label: string
  onSelect?: () => void
}

interface SettingsBreadcrumbsProps {
  ariaLabel: string
  items: readonly SettingsBreadcrumbItem[]
}

export function SettingsBreadcrumbs({ ariaLabel, items }: SettingsBreadcrumbsProps) {
  return (
    <nav className="settings-breadcrumbs" aria-label={ariaLabel}>
      <ol>
        {items.map((item, index) => {
          const isCurrent = index === items.length - 1
          return (
            <li key={item.id}>
              {index > 0 && <ChevronRight aria-hidden="true" />}
              {item.onSelect && !isCurrent ? (
                <button onClick={item.onSelect} type="button">
                  {item.label}
                </button>
              ) : (
                <span aria-current={isCurrent ? 'page' : undefined}>{item.label}</span>
              )}
            </li>
          )
        })}
      </ol>
    </nav>
  )
}
