const GIT_REVIEW_PREFERENCES_KEY = 'mycopilot.gitReview.preferences.v1'

export interface GitReviewPreferences {
  loadFullFiles: boolean
}

const DEFAULT_GIT_REVIEW_PREFERENCES: GitReviewPreferences = {
  loadFullFiles: true
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

/**
 * Review preferences are presentation-only. File contents and repository metadata must never be
 * persisted alongside them.
 */
export function loadGitReviewPreferences(): GitReviewPreferences {
  if (typeof window === 'undefined') return DEFAULT_GIT_REVIEW_PREFERENCES

  try {
    const rawValue = window.localStorage.getItem(GIT_REVIEW_PREFERENCES_KEY)
    if (!rawValue) return DEFAULT_GIT_REVIEW_PREFERENCES
    const value: unknown = JSON.parse(rawValue)
    if (!isRecord(value) || typeof value.loadFullFiles !== 'boolean') {
      return DEFAULT_GIT_REVIEW_PREFERENCES
    }
    return { loadFullFiles: value.loadFullFiles }
  } catch {
    return DEFAULT_GIT_REVIEW_PREFERENCES
  }
}

export function saveGitReviewPreferences(preferences: GitReviewPreferences): void {
  if (typeof window === 'undefined') return

  try {
    window.localStorage.setItem(
      GIT_REVIEW_PREFERENCES_KEY,
      JSON.stringify({ loadFullFiles: preferences.loadFullFiles })
    )
  } catch {
    // Preference persistence is best-effort; Review remains fully usable without localStorage.
  }
}
