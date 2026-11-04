import { Check, Folder, Plus, Search, X } from 'lucide-react'
import type { TranslationKey } from '../../../config/frontendTranslations'
import type { AppProject } from '../../../config/projectConfig'

interface ComposerProjectMenuProps {
  projectSearch: string
  setProjectSearch: (value: string) => void
  filteredProjects: readonly AppProject[]
  selectedProjectId: string | null
  onSelectProject: (project: AppProject) => void
  handleSelectProjectDirectory: () => Promise<void>
  clearSelectedProject: () => void
  t: (key: TranslationKey) => string
}

export function ComposerProjectMenu({
  projectSearch,
  setProjectSearch,
  filteredProjects,
  selectedProjectId,
  onSelectProject,
  handleSelectProjectDirectory,
  clearSelectedProject,
  t
}: ComposerProjectMenuProps) {
  return (
    <div className="composer-project-menu" role="listbox" aria-label={t('project.chooseProject')}>
      <label className="composer-project-menu__search">
        <Search aria-hidden="true" />
        <input
          value={projectSearch}
          placeholder={t('project.searchProject')}
          onChange={(event) => setProjectSearch(event.target.value)}
        />
      </label>

      <div className="composer-project-menu__items">
        {filteredProjects.map((project) => {
          const isSelected = project.id === selectedProjectId

          return (
            <button
              className="composer-project-option"
              data-selected={isSelected || undefined}
              type="button"
              role="option"
              aria-selected={isSelected}
              key={project.id}
              onClick={() => onSelectProject(project)}
            >
              <Folder aria-hidden="true" />
              <span>{project.name}</span>
              {isSelected && <Check aria-hidden="true" />}
            </button>
          )
        })}
      </div>

      <div className="composer-project-menu__divider" />

      <button
        className="composer-project-command"
        type="button"
        onClick={() => {
          void handleSelectProjectDirectory()
        }}
      >
        <Plus aria-hidden="true" />
        <span>{t('project.newProject')}</span>
      </button>

      <button className="composer-project-command" type="button" onClick={clearSelectedProject}>
        <X aria-hidden="true" />
        <span>{t('project.noProject')}</span>
      </button>
    </div>
  )
}
