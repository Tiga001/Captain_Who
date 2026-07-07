// Renderer UI.
import { useState } from "react";
import { UserCircle } from "lucide-react";
import { useFrontendConfig } from "../../../config/FrontendConfigProvider";
import {
  getDefaultProfileDisplayName,
  getProfileDisplayName,
  getProfileHandle,
  getProfileInitials,
  normalizeProfileDisplayName,
} from "../../profile/profileUtils";
import { selectProfileAvatar } from "../../storage/storageClient";
import type { UiPreferencesSnapshot } from "../../storage/storageClient";
import "./ProfileSettingsPage.css";

interface ProfileSettingsPageProps {
  onUiPreferencesChange: (patch: Partial<UiPreferencesSnapshot>) => void;
  uiPreferences: UiPreferencesSnapshot;
}

export function ProfileSettingsPage({ onUiPreferencesChange, uiPreferences }: ProfileSettingsPageProps) {
  const { language, t } = useFrontendConfig();
  const [avatarError, setAvatarError] = useState("");
  const [isRemoveAvatarDialogOpen, setRemoveAvatarDialogOpen] = useState(false);
  const displayName = getProfileDisplayName(uiPreferences, language);
  const handle = getProfileHandle(uiPreferences);
  const initials = getProfileInitials(displayName);
  const explicitDisplayName = normalizeProfileDisplayName(uiPreferences.profileDisplayName);

  const uploadAvatar = async () => {
    setAvatarError("");
    try {
      const avatarDataUrl = await selectProfileAvatar();
      if (!avatarDataUrl) return;
      onUiPreferencesChange({ profileAvatarDataUrl: avatarDataUrl });
    } catch (error) {
      setAvatarError(error instanceof Error ? error.message : t("profile.avatarUploadFailed"));
    }
  };

  return (
    <article className="settings-list-page profile-settings-page">
      <h1>{t("settings.page.profile")}</h1>

      <section className="profile-settings-hero" aria-label={t("profile.account")}>
        <div className="profile-settings-avatar" aria-label={t("profile.avatar")}>
          {uiPreferences.profileAvatarDataUrl ? (
            <img src={uiPreferences.profileAvatarDataUrl} alt="" />
          ) : (
            <span>{initials}</span>
          )}
        </div>
        <div className="profile-settings-hero__text">
          <h2>{displayName}</h2>
          <p>@{handle}</p>
        </div>
      </section>

      <section className="settings-list-section" aria-labelledby="profile-account-heading">
        <h2 id="profile-account-heading">{t("profile.account")}</h2>
        <div className="settings-list">
          <div className="settings-list-row profile-settings-avatar-row">
            <div className="settings-list-row__text">
              <h3 className="settings-list-row__title">{t("profile.avatar")}</h3>
              {avatarError && <p className="profile-settings-error">{avatarError}</p>}
            </div>

            <div className="settings-list-row__control profile-settings-avatar-actions">
              <button className="profile-settings-button" type="button" onClick={uploadAvatar}>
                <UserCircle aria-hidden="true" />
                <span>{t("profile.uploadAvatar")}</span>
              </button>
              {uiPreferences.profileAvatarDataUrl && (
                <button
                  className="profile-settings-button profile-settings-button--secondary"
                  type="button"
                  onClick={() => setRemoveAvatarDialogOpen(true)}
                >
                  {t("profile.removeAvatar")}
                </button>
              )}
            </div>
          </div>

          <label className="settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">{t("profile.displayName")}</span>
            </span>

            <span className="settings-list-row__control">
              <input
                className="settings-list-control"
                value={explicitDisplayName}
                placeholder={getDefaultProfileDisplayName(language)}
                onChange={(event) => onUiPreferencesChange({ profileDisplayName: event.target.value })}
              />
            </span>
          </label>

          <label className="settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">{t("profile.handle")}</span>
            </span>

            <span className="settings-list-row__control">
              <input
                className="settings-list-control"
                value={handle}
                placeholder={t("profile.handlePlaceholder")}
                onChange={(event) => onUiPreferencesChange({ profileHandle: event.target.value })}
              />
            </span>
          </label>
        </div>
      </section>

      {isRemoveAvatarDialogOpen && (
        <div
          className="profile-remove-avatar-dialog"
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="profile-remove-avatar-dialog-title"
          onMouseDown={(event) => {
            if (event.currentTarget === event.target) setRemoveAvatarDialogOpen(false);
          }}
        >
          <div className="profile-remove-avatar-dialog__card">
            <h2 id="profile-remove-avatar-dialog-title">{t("profile.removeAvatarTitle")}</h2>
            <p>{t("profile.removeAvatarDescription")}</p>
            <div className="profile-remove-avatar-dialog__actions">
              <button
                className="profile-remove-avatar-dialog__cancel"
                type="button"
                onClick={() => setRemoveAvatarDialogOpen(false)}
              >
                {t("project.cancel")}
              </button>
              <button
                className="profile-remove-avatar-dialog__confirm"
                type="button"
                onClick={() => {
                  onUiPreferencesChange({ profileAvatarDataUrl: null });
                  setRemoveAvatarDialogOpen(false);
                }}
              >
                {t("profile.confirmRemoveAvatar")}
              </button>
            </div>
          </div>
        </div>
      )}
    </article>
  );
}
