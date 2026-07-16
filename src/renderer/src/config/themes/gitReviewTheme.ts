import type { ColorScheme, GitReviewColors, GitReviewSyntaxColors } from './types'

export interface GitReviewSyntaxPaletteSeed {
  readonly attribute?: string
  readonly background?: string
  readonly comment: string
  readonly constant?: string
  readonly foreground: string
  readonly function: string
  readonly invalid?: string
  readonly keyword: string
  readonly number: string
  readonly regexp?: string
  readonly string: string
  readonly tag?: string
  readonly type: string
  readonly variable: string
}

interface GitReviewThemeSeed {
  readonly colorScheme: ColorScheme
  readonly surface: {
    readonly panel: string
    readonly muted: string
  }
  readonly text: {
    readonly primary: string
    readonly secondary: string
    readonly muted: string
  }
  readonly border: {
    readonly hairline: string
    readonly subtle: string
    readonly default: string
  }
  readonly stateHover: string
  readonly additionText: string
  readonly deletionText: string
  readonly syntax: GitReviewSyntaxPaletteSeed
}

function mix(first: string, firstWeight: number, second: string, colorSpace = 'srgb'): string {
  return `color-mix(in ${colorSpace}, ${first} ${firstWeight}%, ${second})`
}

function createSyntaxColors(
  seed: GitReviewSyntaxPaletteSeed,
  invalidFallback: string
): GitReviewSyntaxColors {
  return {
    attribute: seed.attribute ?? seed.type,
    background: seed.background ?? 'transparent',
    comment: seed.comment,
    constant: seed.constant ?? seed.number,
    foreground: seed.foreground,
    function: seed.function,
    invalid: seed.invalid ?? invalidFallback,
    keyword: seed.keyword,
    number: seed.number,
    regexp: seed.regexp ?? seed.string,
    string: seed.string,
    tag: seed.tag ?? seed.keyword,
    type: seed.type,
    variable: seed.variable
  }
}

export function createGitReviewColors(seed: GitReviewThemeSeed): GitReviewColors {
  const isDark = seed.colorScheme === 'dark'
  const { panel, muted } = seed.surface

  return {
    surface: {
      panel,
      fileList: mix(panel, 88, muted),
      card: 'transparent',
      header: panel,
      headerHover: `linear-gradient(${seed.stateHover}, ${seed.stateHover}), ${panel}`,
      headerExpanded: `linear-gradient(${mix(seed.stateHover, 62, 'transparent')}, ${mix(seed.stateHover, 62, 'transparent')}), ${panel}`,
      gutter: mix(muted, 45, panel),
      bufferGutter: mix(muted, 58, panel),
      addition: mix(panel, isDark ? 80 : 88, seed.additionText, 'lab'),
      deletion: mix(panel, isDark ? 80 : 88, seed.deletionText, 'lab'),
      additionGutter: mix(panel, isDark ? 85 : 91, seed.additionText, 'lab'),
      deletionGutter: mix(panel, isDark ? 85 : 91, seed.deletionText, 'lab'),
      gap: mix(muted, 78, panel),
      gapGutter: mix(muted, 88, 'transparent'),
      buffer: mix(muted, 32, panel)
    },
    text: {
      primary: seed.text.primary,
      secondary: seed.text.secondary,
      muted: seed.text.muted,
      lineNumber: seed.text.muted,
      meta: seed.text.muted,
      addition: seed.additionText,
      deletion: seed.deletionText
    },
    border: {
      default: seed.border.default,
      subtle: seed.border.subtle,
      rowDivider: mix(seed.border.hairline, 55, 'transparent')
    },
    bufferStripe: mix(seed.text.muted, 28, 'transparent'),
    syntax: createSyntaxColors(seed.syntax, 'var(--mc-color-text-danger)')
  }
}

export function getGitReviewSyntaxCssVariables(
  syntax: GitReviewSyntaxColors
): Record<string, string> {
  return {
    '--git-review-syntax-attribute': syntax.attribute,
    '--git-review-syntax-background': syntax.background,
    '--git-review-syntax-comment': syntax.comment,
    '--git-review-syntax-constant': syntax.constant,
    '--git-review-syntax-foreground': syntax.foreground,
    '--git-review-syntax-function': syntax.function,
    '--git-review-syntax-invalid': syntax.invalid,
    '--git-review-syntax-keyword': syntax.keyword,
    '--git-review-syntax-number': syntax.number,
    '--git-review-syntax-regexp': syntax.regexp,
    '--git-review-syntax-string': syntax.string,
    '--git-review-syntax-tag': syntax.tag,
    '--git-review-syntax-type': syntax.type,
    '--git-review-syntax-variable': syntax.variable
  }
}

export function getGitReviewCssVariables(colors: GitReviewColors): Record<string, string> {
  return {
    '--mc-color-git-review-panel-surface': colors.surface.panel,
    '--mc-color-git-review-file-list-surface': colors.surface.fileList,
    '--mc-color-git-review-card-surface': colors.surface.card,
    '--mc-color-git-review-header-surface': colors.surface.header,
    '--mc-color-git-review-header-hover-surface': colors.surface.headerHover,
    '--mc-color-git-review-header-expanded-surface': colors.surface.headerExpanded,
    '--mc-color-git-review-gutter-surface': colors.surface.gutter,
    '--mc-color-git-review-buffer-gutter-surface': colors.surface.bufferGutter,
    '--mc-color-git-review-addition-surface': colors.surface.addition,
    '--mc-color-git-review-deletion-surface': colors.surface.deletion,
    '--mc-color-git-review-addition-gutter-surface': colors.surface.additionGutter,
    '--mc-color-git-review-deletion-gutter-surface': colors.surface.deletionGutter,
    '--mc-color-git-review-gap-surface': colors.surface.gap,
    '--mc-color-git-review-gap-gutter-surface': colors.surface.gapGutter,
    '--mc-color-git-review-buffer-surface': colors.surface.buffer,
    '--mc-color-git-review-text-primary': colors.text.primary,
    '--mc-color-git-review-text-secondary': colors.text.secondary,
    '--mc-color-git-review-text-muted': colors.text.muted,
    '--mc-color-git-review-line-number-text': colors.text.lineNumber,
    '--mc-color-git-review-meta-text': colors.text.meta,
    '--mc-color-git-review-addition-text': colors.text.addition,
    '--mc-color-git-review-deletion-text': colors.text.deletion,
    '--mc-color-git-review-border-default': colors.border.default,
    '--mc-color-git-review-border-subtle': colors.border.subtle,
    '--mc-color-git-review-row-divider': colors.border.rowDivider,
    '--mc-color-git-review-buffer-stripe': colors.bufferStripe,
    ...getGitReviewSyntaxCssVariables(colors.syntax)
  }
}
