// Renderer UI.
import type { AppLanguage } from "../../config/frontendTranslations";
import type { UiPreferencesSnapshot } from "../storage/storageClient";

const LEGACY_PROFILE_DISPLAY_NAME = "hx z";
const LEGACY_PROFILE_HANDLE = "hxz9393";
const DEFAULT_PROFILE_HANDLE = "USER";

export function getDefaultProfileDisplayName(language: AppLanguage) {
  return language === "zh-CN" ? "用户" : "USER";
}

export function normalizeProfileDisplayName(value: string | null | undefined) {
  if (typeof value !== "string") return "";
  const normalizedValue = value.trim();
  if (normalizedValue.toLowerCase() === LEGACY_PROFILE_DISPLAY_NAME) return "";
  return normalizedValue;
}

export function normalizeProfileHandle(value: string | null | undefined) {
  if (typeof value !== "string") return DEFAULT_PROFILE_HANDLE;
  const normalizedValue = value.trim().replace(/^@+/, "");
  if (!normalizedValue || normalizedValue.toLowerCase() === LEGACY_PROFILE_HANDLE) return DEFAULT_PROFILE_HANDLE;
  return normalizedValue;
}

export function getProfileDisplayName(uiPreferences: UiPreferencesSnapshot, language: AppLanguage) {
  return normalizeProfileDisplayName(uiPreferences.profileDisplayName) || getDefaultProfileDisplayName(language);
}

export function getProfileHandle(uiPreferences: UiPreferencesSnapshot) {
  return normalizeProfileHandle(uiPreferences.profileHandle);
}

export function getProfileInitials(displayName: string) {
  const characters = Array.from(displayName.trim()).filter((character) => !/\s/u.test(character));
  if (characters.some((character) => /\p{Script=Han}/u.test(character))) {
    return characters.slice(-2).join("") || DEFAULT_PROFILE_HANDLE.slice(0, 2);
  }

  return (characters.slice(0, 2).join("") || DEFAULT_PROFILE_HANDLE.slice(0, 2)).toUpperCase();
}
