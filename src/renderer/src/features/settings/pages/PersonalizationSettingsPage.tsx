// Renderer UI.
import { useEffect, useMemo, useState } from "react";
import { Check, ChevronDown, MessageCircle, Terminal } from "lucide-react";
import { useFrontendConfig } from "../../../config/FrontendConfigProvider";
import {
  defaultAgentPromptPreferences,
  loadAgentPromptPreferences,
  saveAgentPromptPreferences,
} from "../../storage/storageClient";
import type { AgentPromptPreferencesSnapshot } from "../../storage/storageClient";
import "./PersonalizationSettingsPage.css";

type PromptWorkMode = AgentPromptPreferencesSnapshot["workMode"];
type PromptTone = AgentPromptPreferencesSnapshot["tone"];

const WORK_MODE_OPTIONS: Array<{
  value: PromptWorkMode;
  titleKey: "personalization.workModeCoding" | "personalization.workModeGeneral";
  descriptionKey: "personalization.workModeCodingDescription" | "personalization.workModeGeneralDescription";
  icon: typeof Terminal;
}> = [
  {
    value: "coding",
    titleKey: "personalization.workModeCoding",
    descriptionKey: "personalization.workModeCodingDescription",
    icon: Terminal,
  },
  {
    value: "general",
    titleKey: "personalization.workModeGeneral",
    descriptionKey: "personalization.workModeGeneralDescription",
    icon: MessageCircle,
  },
];

const TONE_OPTIONS: Array<{
  value: PromptTone;
  titleKey: "personalization.toneFriendly" | "personalization.tonePragmatic";
  descriptionKey: "personalization.toneFriendlyDescription" | "personalization.tonePragmaticDescription";
}> = [
  {
    value: "friendly",
    titleKey: "personalization.toneFriendly",
    descriptionKey: "personalization.toneFriendlyDescription",
  },
  {
    value: "pragmatic",
    titleKey: "personalization.tonePragmatic",
    descriptionKey: "personalization.tonePragmaticDescription",
  },
];

function getComparablePreferences(preferences: AgentPromptPreferencesSnapshot) {
  return {
    workMode: preferences.workMode,
    tone: preferences.tone,
    detailLevel: preferences.detailLevel,
    customInstructions: preferences.customInstructions,
  };
}

export function PersonalizationSettingsPage() {
  const { t } = useFrontendConfig();
  const [preferences, setPreferences] = useState<AgentPromptPreferencesSnapshot>(() => defaultAgentPromptPreferences());
  const [savedPreferences, setSavedPreferences] = useState<AgentPromptPreferencesSnapshot>(() =>
    defaultAgentPromptPreferences(),
  );
  const [isLoading, setIsLoading] = useState(true);
  const [isSaving, setIsSaving] = useState(false);
  const [statusMessage, setStatusMessage] = useState("");
  const [isToneOpen, setToneOpen] = useState(false);

  useEffect(() => {
    let isCancelled = false;

    async function loadPreferences() {
      setIsLoading(true);
      setStatusMessage("");
      try {
        const loadedPreferences = await loadAgentPromptPreferences();
        if (isCancelled) return;
        setPreferences(loadedPreferences);
        setSavedPreferences(loadedPreferences);
      } catch {
        if (!isCancelled) setStatusMessage(t("personalization.loadFailed"));
      } finally {
        if (!isCancelled) setIsLoading(false);
      }
    }

    void loadPreferences();

    return () => {
      isCancelled = true;
    };
  }, [t]);

  const selectedTone = useMemo(
    () => TONE_OPTIONS.find((option) => option.value === preferences.tone) ?? TONE_OPTIONS[1],
    [preferences.tone],
  );
  const isDirty =
    JSON.stringify(getComparablePreferences(preferences)) !== JSON.stringify(getComparablePreferences(savedPreferences));

  const updatePreferences = (patch: Partial<AgentPromptPreferencesSnapshot>) => {
    setStatusMessage("");
    setPreferences((current) => ({ ...current, ...patch }));
  };

  const savePreferences = async () => {
    if (!isDirty || isSaving) return;

    setIsSaving(true);
    setStatusMessage("");
    try {
      const saved = await saveAgentPromptPreferences({
        workMode: preferences.workMode,
        tone: preferences.tone,
        detailLevel: preferences.detailLevel || "medium",
        customInstructions: preferences.customInstructions,
      });
      setPreferences(saved);
      setSavedPreferences(saved);
    } catch {
      setStatusMessage(t("personalization.saveFailed"));
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <article className="settings-list-page personalization-settings-page">
      <h1>{t("settings.page.personalization")}</h1>

      <section className="personalization-work-mode" aria-labelledby="personalization-work-mode-heading">
        <div className="personalization-section-heading">
          <h2 id="personalization-work-mode-heading">{t("personalization.workMode")}</h2>
          <p>{t("personalization.workModeDescription")}</p>
        </div>

        <div className="personalization-work-mode__grid">
          {WORK_MODE_OPTIONS.map((option) => {
            const Icon = option.icon;
            const isSelected = preferences.workMode === option.value;

            return (
              <button
                className="personalization-work-mode-card"
                data-selected={isSelected || undefined}
                type="button"
                key={option.value}
                onClick={() => updatePreferences({ workMode: option.value })}
              >
                <Icon aria-hidden="true" />
                <span className="personalization-work-mode-card__text">
                  <strong>{t(option.titleKey)}</strong>
                  <span>{t(option.descriptionKey)}</span>
                </span>
                <span className="personalization-radio" data-selected={isSelected || undefined} aria-hidden="true" />
              </button>
            );
          })}
        </div>
      </section>

      <section className="settings-list-section personalization-tone-section" aria-labelledby="personalization-tone-heading">
        <div className="settings-list personalization-tone-list">
          <div className="settings-list-row personalization-tone-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title" id="personalization-tone-heading">
                {t("personalization.tone")}
              </span>
              <span className="settings-list-row__description">{t("personalization.toneDescription")}</span>
            </span>

            <span className="settings-list-row__control personalization-tone-control">
              <button
                className="personalization-tone-button"
                type="button"
                aria-haspopup="listbox"
                aria-expanded={isToneOpen}
                onClick={() => setToneOpen((current) => !current)}
              >
                <span>{t(selectedTone.titleKey)}</span>
                <ChevronDown aria-hidden="true" />
              </button>

              {isToneOpen && (
                <div className="personalization-tone-menu" role="listbox" aria-label={t("personalization.tone")}>
                  {TONE_OPTIONS.map((option) => {
                    const isSelected = preferences.tone === option.value;
                    return (
                      <button
                        className="personalization-tone-option"
                        data-selected={isSelected || undefined}
                        type="button"
                        role="option"
                        aria-selected={isSelected}
                        key={option.value}
                        onClick={() => {
                          updatePreferences({ tone: option.value });
                          setToneOpen(false);
                        }}
                      >
                        <span>
                          <strong>{t(option.titleKey)}</strong>
                          <small>{t(option.descriptionKey)}</small>
                        </span>
                        {isSelected && <Check aria-hidden="true" />}
                      </button>
                    );
                  })}
                </div>
              )}
            </span>
          </div>
        </div>
      </section>

      <section className="personalization-custom-instructions" aria-labelledby="personalization-custom-heading">
        <div className="personalization-section-heading">
          <h2 id="personalization-custom-heading">{t("personalization.customInstructions")}</h2>
          <p>{t("personalization.customInstructionsDescription")}</p>
        </div>

        <textarea
          className="personalization-custom-instructions__textarea"
          value={preferences.customInstructions}
          placeholder={t("personalization.customInstructionsPlaceholder")}
          onChange={(event) => updatePreferences({ customInstructions: event.target.value })}
        />
      </section>

      <div className="personalization-actions">
        {statusMessage && <p className="personalization-status">{statusMessage}</p>}
        <button
          className="personalization-save-button"
          type="button"
          disabled={isLoading || isSaving || !isDirty}
          onClick={savePreferences}
        >
          {t("personalization.save")}
        </button>
      </div>
    </article>
  );
}
