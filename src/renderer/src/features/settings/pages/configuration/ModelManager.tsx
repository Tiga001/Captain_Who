import { Pencil, Plus, Trash2 } from 'lucide-react'
import { useState } from 'react'
import type { ReactElement } from 'react'
import { ConfirmationDialog } from '../../../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { ModelConfig } from './configurationTypes'
import { formatContextWindow } from './modelPresentation'

interface ModelManagerProps {
  models: ModelConfig[]
  onBack: () => void
  onCreate: () => void
  onDelete: (modelId: string) => void
  onEdit: (model: ModelConfig) => void
}

export function ModelManager({
  models,
  onBack,
  onCreate,
  onDelete,
  onEdit
}: ModelManagerProps): ReactElement {
  const { t } = useFrontendConfig()
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null)
  const pendingDeleteModel = models.find((model) => model.id === pendingDeleteId)

  return (
    <section className="model-manager-page" aria-labelledby="model-manager-heading">
      <div className="model-manager-page__header">
        <h1 id="model-manager-heading">{t('configuration.modelManager')}</h1>

        <div className="model-manager-page__header-actions">
          <button className="secondary-settings-button" type="button" onClick={onBack}>
            {t('configuration.done')}
          </button>
          <button
            className="secondary-settings-button secondary-settings-button--accent"
            type="button"
            onClick={onCreate}
          >
            <Plus aria-hidden="true" />
            <span>{t('configuration.newModel')}</span>
          </button>
        </div>
      </div>

      <div className="model-manager-table" role="table" aria-label={t('configuration.modelTable')}>
        <div className="model-manager-table__row model-manager-table__row--head" role="row">
          <span role="columnheader">{t('configuration.tableModel')}</span>
          <span role="columnheader">{t('configuration.tableContext')}</span>
          <span role="columnheader">{t('configuration.tableImage')}</span>
          <span role="columnheader">{t('configuration.tableInput')}</span>
          <span role="columnheader">{t('configuration.tableOutput')}</span>
          <span role="columnheader" aria-label={t('configuration.tableActions')} />
        </div>

        {models.map((model) => (
          <div className="model-manager-table__row" role="row" key={model.id}>
            <span className="model-manager-table__model" role="cell">
              <strong>{model.displayName}</strong>
              {model.providerPath && <small>{model.providerPath}</small>}
            </span>
            <span
              className="model-manager-table__context"
              role="cell"
              title={
                model.contextWindowTokens
                  ? `${model.contextWindowTokens.toLocaleString()} tokens`
                  : t('configuration.contextNotConfigured')
              }
            >
              {formatContextWindow(model.contextWindowTokens)}
            </span>
            <span className="model-manager-table__image" role="cell">
              <span
                className="image-support-pill"
                data-supported={model.supportsImage || undefined}
              >
                {model.supportsImage
                  ? t('configuration.supported')
                  : t('configuration.unsupported')}
              </span>
            </span>
            <span className="model-manager-table__price" role="cell">
              {model.inputPrice}
            </span>
            <span className="model-manager-table__price" role="cell">
              {model.outputPrice}
            </span>
            <span className="model-manager-table__actions" role="cell">
              <button
                className="model-manager-table__icon-button"
                type="button"
                title={t('configuration.edit')}
                aria-label={`${t('configuration.edit')}: ${model.displayName}`}
                onClick={() => onEdit(model)}
              >
                <Pencil aria-hidden="true" />
              </button>
              <button
                className="model-manager-table__icon-button model-manager-table__icon-button--danger"
                type="button"
                title={t('configuration.delete')}
                aria-label={`${t('configuration.delete')}: ${model.displayName}`}
                onClick={() => setPendingDeleteId(model.id)}
              >
                <Trash2 aria-hidden="true" />
              </button>
            </span>
          </div>
        ))}
      </div>

      {pendingDeleteModel && (
        <ConfirmationDialog
          title={t('configuration.confirmDelete')}
          description={pendingDeleteModel.displayName}
          cancelLabel={t('configuration.cancel')}
          confirmLabel={t('configuration.delete')}
          onCancel={() => setPendingDeleteId(null)}
          onConfirm={() => {
            onDelete(pendingDeleteModel.id)
            setPendingDeleteId(null)
          }}
        />
      )}
    </section>
  )
}
