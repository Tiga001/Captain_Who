import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import type { RightSidebarModuleDefinition, RightSidebarModuleId } from './rightSidebarTypes'

interface RightSidebarHomeProps {
  modules: RightSidebarModuleDefinition[]
  onOpenModule: (moduleId: RightSidebarModuleId) => void
}

export function RightSidebarHome({ modules, onOpenModule }: RightSidebarHomeProps) {
  const { t } = useFrontendConfig()

  return (
    <nav className="right-sidebar__home" aria-label={t('rightSidebar.tools')}>
      {modules.map((module) => {
        const Icon = module.icon

        return (
          <button
            className="right-sidebar__tool-card"
            type="button"
            key={module.id}
            onClick={() => onOpenModule(module.id)}
          >
            <span className="right-sidebar__tool-heading">
              <Icon aria-hidden="true" />
              <span className="right-sidebar__tool-title">{t(module.titleKey)}</span>
              {module.badge ? (
                <span className="right-sidebar__module-badge" aria-label={`${module.badge}`}>
                  {module.badge}
                </span>
              ) : null}
            </span>
          </button>
        )
      })}
    </nav>
  )
}
