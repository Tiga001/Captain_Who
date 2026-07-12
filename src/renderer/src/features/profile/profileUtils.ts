import type { UiPreferencesSnapshot } from '../storage/storageClient'

const DEFAULT_PROFILE_HANDLE = 'USER'

export function normalizeProfileDisplayName(value: string | null | undefined) {
  if (typeof value !== 'string') return ''
  return value.trim()
}

function normalizeProfileHandle(value: string | null | undefined) {
  if (typeof value !== 'string') return DEFAULT_PROFILE_HANDLE
  const normalizedValue = value.trim().replace(/^@+/, '')
  return normalizedValue || DEFAULT_PROFILE_HANDLE
}

export function getProfileDisplayName(
  uiPreferences: UiPreferencesSnapshot,
  defaultDisplayName: string
) {
  return normalizeProfileDisplayName(uiPreferences.profileDisplayName) || defaultDisplayName
}

export function getProfileHandle(uiPreferences: UiPreferencesSnapshot) {
  return normalizeProfileHandle(uiPreferences.profileHandle)
}

export function getProfileInitials(displayName: string) {
  const characters = Array.from(displayName.trim()).filter((character) => !/\s/u.test(character))
  if (characters.some((character) => /\p{Script=Han}/u.test(character))) {
    return characters.slice(-2).join('') || DEFAULT_PROFILE_HANDLE.slice(0, 2)
  }

  return (characters.slice(0, 2).join('') || DEFAULT_PROFILE_HANDLE.slice(0, 2)).toUpperCase()
}
