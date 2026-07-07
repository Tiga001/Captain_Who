// Renderer UI.
import { useMemo, useState } from "react";
import { useFrontendConfig } from "../../../../config/FrontendConfigProvider";
import type { ModelConfig, ModelFormValues } from "./configurationTypes";

interface ModelFormProps {
  model?: ModelConfig;
  onCancel: () => void;
  onSave: (values: ModelFormValues) => void;
}

function toFormValues(model?: ModelConfig): ModelFormValues {
  return {
    id: model?.id ?? "",
    displayName: model?.displayName ?? "",
    inputPrice: model?.inputPrice ?? "0",
    outputPrice: model?.outputPrice ?? "0",
    supportsImage: model?.supportsImage ?? false,
  };
}

export function ModelForm({ model, onCancel, onSave }: ModelFormProps) {
  const { t } = useFrontendConfig();
  const initialValues = useMemo(() => toFormValues(model), [model]);
  const [values, setValues] = useState<ModelFormValues>(initialValues);
  const isEditing = Boolean(model);
  const canSave = values.id.trim().length > 0;

  return (
    <form
      className="model-form-page"
      aria-labelledby="model-form-heading"
      onSubmit={(event) => {
        event.preventDefault();
        if (!canSave) return;
        onSave({
          ...values,
          id: values.id.trim(),
          displayName: values.displayName.trim(),
          inputPrice: values.inputPrice.trim() || "0",
          outputPrice: values.outputPrice.trim() || "0",
        });
      }}
    >
      <h1 id="model-form-heading">{isEditing ? t("configuration.editModel") : t("configuration.newModel")}</h1>

      <div className="model-form-page__fields settings-list">
        <label className="configuration-field settings-list-row">
          <span className="settings-list-row__text">
            <span className="settings-list-row__title">{t("configuration.modelId")}</span>
          </span>
          <span className="settings-list-row__control">
            <input
              className="settings-list-control"
              value={values.id}
              placeholder={t("configuration.modelIdPlaceholder")}
              onChange={(event) => setValues((current) => ({ ...current, id: event.target.value }))}
            />
          </span>
        </label>

        <label className="configuration-field settings-list-row">
          <span className="settings-list-row__text">
            <span className="settings-list-row__title">{t("configuration.displayName")}</span>
          </span>
          <span className="settings-list-row__control">
            <input
              className="settings-list-control"
              value={values.displayName}
              placeholder={t("configuration.displayNamePlaceholder")}
              onChange={(event) => setValues((current) => ({ ...current, displayName: event.target.value }))}
            />
          </span>
        </label>

        <label className="configuration-field settings-list-row">
          <span className="settings-list-row__text">
            <span className="settings-list-row__title">{t("configuration.inputPrice")}</span>
          </span>
          <span className="settings-list-row__control">
            <input
              className="settings-list-control"
              inputMode="decimal"
              value={values.inputPrice}
              onChange={(event) => setValues((current) => ({ ...current, inputPrice: event.target.value }))}
            />
          </span>
        </label>

        <label className="configuration-field settings-list-row">
          <span className="settings-list-row__text">
            <span className="settings-list-row__title">{t("configuration.outputPrice")}</span>
          </span>
          <span className="settings-list-row__control">
            <input
              className="settings-list-control"
              inputMode="decimal"
              value={values.outputPrice}
              onChange={(event) => setValues((current) => ({ ...current, outputPrice: event.target.value }))}
            />
          </span>
        </label>

        <div className="configuration-field settings-list-row">
          <span className="settings-list-row__text">
            <span className="settings-list-row__title">{t("configuration.supportsImageInput")}</span>
          </span>
          <button
            className="settings-switch"
            type="button"
            role="switch"
            aria-checked={values.supportsImage}
            data-state={values.supportsImage ? "on" : "off"}
            onClick={() => setValues((current) => ({ ...current, supportsImage: !current.supportsImage }))}
          >
            <span className="settings-switch__thumb" aria-hidden="true" />
            <span className="sr-only">{t("configuration.supportsImageInput")}</span>
          </button>
        </div>
      </div>

      <div className="model-form-page__actions">
        <button className="secondary-settings-button" type="button" onClick={onCancel}>
          {t("configuration.cancel")}
        </button>
        <button className="primary-settings-button" type="submit" disabled={!canSave}>
          {t("configuration.save")}
        </button>
      </div>
    </form>
  );
}
