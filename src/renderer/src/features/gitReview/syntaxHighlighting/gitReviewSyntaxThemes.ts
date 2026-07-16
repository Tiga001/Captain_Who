import type { CSSProperties } from 'react'
import {
  frontendThemes,
  getFrontendTheme,
  type FrontendThemeId,
  type GitReviewSyntaxColors
} from '../../../config/frontendTheme'
import { getGitReviewSyntaxCssVariables } from '../../../config/themes/gitReviewTheme'

export type GitReviewSyntaxPalette = GitReviewSyntaxColors

export const syntaxPaletteIdByFrontendThemeId = Object.freeze(
  Object.fromEntries(
    (Object.keys(frontendThemes) as FrontendThemeId[]).map((themeId) => [themeId, themeId])
  ) as Record<FrontendThemeId, FrontendThemeId>
)

const syntaxThemeStyles = Object.freeze(
  Object.fromEntries(
    (Object.keys(frontendThemes) as FrontendThemeId[]).map((themeId) => [
      themeId,
      Object.freeze(
        getGitReviewSyntaxCssVariables(getFrontendTheme(themeId).tokens.colors.gitReview.syntax)
      ) as CSSProperties
    ])
  ) as Record<FrontendThemeId, CSSProperties>
)

/**
 * Compatibility adapter for isolated renderer tests. Production receives these variables from the
 * global frontend theme, so the review feature no longer owns a parallel theme registry.
 */
export function getGitReviewSyntaxThemeStyle(themeId: FrontendThemeId): CSSProperties {
  return syntaxThemeStyles[themeId]
}
