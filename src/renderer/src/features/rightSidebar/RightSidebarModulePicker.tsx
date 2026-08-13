import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import type { CSSProperties } from 'react'
import type { RightSidebarModuleDefinition, RightSidebarModuleId } from './rightSidebarTypes'

interface RightSidebarModulePickerProps {
  modules: RightSidebarModuleDefinition[]
  onOpenModule: (moduleId: RightSidebarModuleId) => void
  style?: CSSProperties
}

export function RightSidebarModulePicker({
  modules,
  onOpenModule,
  style
}: RightSidebarModulePickerProps) {
  const { t } = useFrontendConfig()

  return (
    <div
      className="right-sidebar__module-menu"
      role="menu"
      aria-label={t('rightSidebar.tools')}
      style={style}
    >
      {modules.map((module) => {
        const Icon = module.icon

        return (
          <button
            className="right-sidebar__module-menu-item"
            type="button"
            role="menuitem"
            key={module.id}
            onClick={() => onOpenModule(module.id)}
          >
            <Icon aria-hidden="true" />
            <span>{t(module.titleKey)}</span>
            {module.badge ? (
              <span className="right-sidebar__module-badge" aria-label={`${module.badge}`}>
                {module.badge}
              </span>
            ) : null}
          </button>
        )
      })}
    </div>
  )
}
