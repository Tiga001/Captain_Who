// Renderer UI.
import { useFrontendConfig } from "../../../config/FrontendConfigProvider";
import { isMacOS } from "../../../lib/platform";
import type { ThemePreference } from "../../../config/frontendTheme";
import type { TranslationKey } from "../../../config/frontendTranslations";
import type { UiPreferencesSnapshot } from "../../storage/storageClient";
import {
  MAX_TRANSLUCENT_SIDEBAR_TRANSPARENCY,
  MIN_TRANSLUCENT_SIDEBAR_TRANSPARENCY,
  normalizeTranslucentSidebarTransparency,
} from "../../storage/storageClient";
import "./AppearanceSettingsPage.css";

const THEME_OPTIONS: Array<{
  id: ThemePreference;
  labelKey: TranslationKey;
  preview: "system" | "light" | "dark";
}> = [
  { id: "system", labelKey: "appearance.theme.system", preview: "system" },
  { id: "light", labelKey: "appearance.theme.light", preview: "light" },
  { id: "dark", labelKey: "appearance.theme.dark", preview: "dark" },
];

const SUPPORTS_NATIVE_FONT_SMOOTHING = isMacOS();

interface AppearanceSettingsPageProps {
  onUiPreferencesChange: (patch: Partial<UiPreferencesSnapshot>) => void;
  uiPreferences: UiPreferencesSnapshot;
}

export function AppearanceSettingsPage({
  onUiPreferencesChange,
  uiPreferences,
}: AppearanceSettingsPageProps) {
  const { setThemePreference, t, themePreference } = useFrontendConfig();
  const sidebarTransparency = normalizeTranslucentSidebarTransparency(
    uiPreferences.translucentSidebarTransparency,
  );

  return (
    <article className="settings-list-page appearance-settings-page">
      <h1>{t("settings.page.appearance")}</h1>

      <div className="appearance-theme-grid" role="group" aria-label={t("appearance.theme")}>
        {THEME_OPTIONS.map((option) => (
          <button
            className="appearance-theme-option"
            type="button"
            data-active={themePreference === option.id || undefined}
            data-theme-option={option.preview}
            aria-pressed={themePreference === option.id}
            key={option.id}
            onClick={() => setThemePreference(option.id)}
          >
            <span
              className="appearance-theme-preview"
              data-preview={option.preview}
              aria-hidden="true"
            >
              <span className="appearance-theme-preview__window" />
              <span className="appearance-theme-preview__header">
                <span />
                <span />
              </span>
              <span className="appearance-theme-preview__panel">
                <span />
                <span />
                <span />
              </span>
            </span>
            <span className="appearance-theme-option__label">{t(option.labelKey)}</span>
          </button>
        ))}
      </div>

      <section
        className="settings-list-section appearance-settings-section"
        aria-labelledby="translucent-sidebar-heading"
      >
        <div className="settings-list">
          {SUPPORTS_NATIVE_FONT_SMOOTHING && (
            <div className="settings-list-row">
              <div className="settings-list-row__text">
                <h2 className="settings-list-row__title" id="native-font-smoothing-heading">
                  {t("appearance.nativeFontSmoothing")}
                </h2>
                <p className="settings-list-row__description">
                  {t("appearance.nativeFontSmoothingDescription")}
                </p>
              </div>

              <button
                className="settings-switch appearance-settings-switch"
                type="button"
                role="switch"
                aria-checked={uiPreferences.nativeFontSmoothing}
                data-state={uiPreferences.nativeFontSmoothing ? "on" : "off"}
                onClick={() =>
                  onUiPreferencesChange({ nativeFontSmoothing: !uiPreferences.nativeFontSmoothing })
                }
              >
                <span className="settings-switch__thumb" />
              </button>
            </div>
          )}

          <div className="settings-list-row">
            <div className="settings-list-row__text">
              <h2 className="settings-list-row__title" id="translucent-sidebar-heading">
                {t("appearance.translucentSidebar")}
              </h2>
              <p className="settings-list-row__description">
                {t("appearance.translucentSidebarDescription")}
              </p>
            </div>

            <button
              className="settings-switch appearance-settings-switch"
              type="button"
              role="switch"
              aria-checked={uiPreferences.translucentSidebar}
              data-state={uiPreferences.translucentSidebar ? "on" : "off"}
              onClick={() =>
                onUiPreferencesChange({ translucentSidebar: !uiPreferences.translucentSidebar })
              }
            >
              <span className="settings-switch__thumb" />
            </button>
          </div>

          <div
            className="appearance-translucency-drawer"
            data-open={uiPreferences.translucentSidebar ? "true" : "false"}
            aria-hidden={!uiPreferences.translucentSidebar}
          >
            <div className="settings-list-row appearance-translucency-row">
              <div className="settings-list-row__text">
                <h2
                  className="settings-list-row__title"
                  id="translucent-sidebar-transparency-heading"
                >
                  {t("appearance.translucentSidebarTransparency")}
                </h2>
              </div>

              <label className="appearance-translucency-control">
                <input
                  type="range"
                  min={MIN_TRANSLUCENT_SIDEBAR_TRANSPARENCY}
                  max={MAX_TRANSLUCENT_SIDEBAR_TRANSPARENCY}
                  step={1}
                  value={sidebarTransparency}
                  aria-labelledby="translucent-sidebar-transparency-heading"
                  disabled={!uiPreferences.translucentSidebar}
                  onChange={(event) =>
                    onUiPreferencesChange({
                      translucentSidebarTransparency: normalizeTranslucentSidebarTransparency(
                        Number(event.currentTarget.value),
                      ),
                    })
                  }
                />
              </label>
            </div>
          </div>
        </div>
      </section>
    </article>
  );
}
