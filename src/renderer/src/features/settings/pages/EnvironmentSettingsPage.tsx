import { renderSettingsNodes, settingLabel } from '../settingsDefinition'
import {
  environmentSettings,
  environmentAddProjectSettings,
  environmentProjectActions
} from './managementSettings.definition'
import { NotebookText, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { useProjectSettings } from '../../../config/ProjectSettingsProvider'
import type { AppProject } from '../../../config/projectConfig'
import './EnvironmentSettingsPage.css'

export function EnvironmentSettingsPage({
  onRemoveProject
}: {
  onRemoveProject: (projectId: string) => Promise<boolean>
}) {
  const { t } = useFrontendConfig()
  const { projects, selectProjectDirectory } = useProjectSettings()
  const [pendingDeleteProject, setPendingDeleteProject] = useState<AppProject | null>(null)

  const addProjectFromFolder = () => {
    void selectProjectDirectory()
  }

  return (
    <article className="settings-list-page environment-settings-page">
      <h1>{t('settings.page.environment')}</h1>

      {renderSettingsNodes(environmentSettings, (node) => (
        <section
          className="settings-list-section environment-projects"
          aria-labelledby="environment-projects-heading"
        >
          <div className="settings-list-section__header environment-projects__header">
            <h2 id="environment-projects-heading">{settingLabel(node, t)}</h2>
            {renderSettingsNodes(environmentAddProjectSettings, (item) => (
              <button
                className="environment-projects__add-button"
                type="button"
                onClick={addProjectFromFolder}
              >
                {settingLabel(item, t)}
              </button>
            ))}
          </div>

          <div className="settings-list environment-projects__list">
            {projects.map((project) => (
              <div className="environment-project-card" key={project.id}>
                <NotebookText aria-hidden="true" />
                <span className="environment-project-card__name">{project.name}</span>
                {project.path && (
                  <span className="environment-project-card__detail">{project.path}</span>
                )}
                {renderSettingsNodes(environmentProjectActions, (item) => (
                  <button
                    className="environment-project-card__delete"
                    type="button"
                    aria-label={`${settingLabel(item, t)} ${project.name}`}
                    onClick={() => setPendingDeleteProject(project)}
                  >
                    <Trash2 aria-hidden="true" />
                  </button>
                ))}
              </div>
            ))}
          </div>
        </section>
      ))}

      {pendingDeleteProject && (
        <ConfirmationDialog
          title={t('environment.removeProjectTitle').replace(
            '{projectName}',
            pendingDeleteProject.name
          )}
          description={t('environment.removeProjectDescription')}
          cancelLabel={t('environment.cancelRemoveProject')}
          confirmLabel={t('environment.confirmRemoveProject')}
          onCancel={() => setPendingDeleteProject(null)}
          onConfirm={async () => {
            if (await onRemoveProject(pendingDeleteProject.id)) {
              setPendingDeleteProject(null)
            }
          }}
        />
      )}
    </article>
  )
}
