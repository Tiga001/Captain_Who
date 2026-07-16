import type { TranslationKey } from '../frontendTranslations'

export type ColorScheme = 'light' | 'dark'
export type ColorSchemePreference = 'system' | ColorScheme

export interface GitReviewSyntaxColors {
  readonly attribute: string
  readonly background: string
  readonly comment: string
  readonly constant: string
  readonly foreground: string
  readonly function: string
  readonly invalid: string
  readonly keyword: string
  readonly number: string
  readonly regexp: string
  readonly string: string
  readonly tag: string
  readonly type: string
  readonly variable: string
}

export interface GitReviewColors {
  readonly surface: {
    readonly panel: string
    readonly fileList: string
    readonly card: string
    readonly header: string
    readonly headerHover: string
    readonly headerExpanded: string
    readonly gutter: string
    readonly bufferGutter: string
    readonly addition: string
    readonly deletion: string
    readonly additionGutter: string
    readonly deletionGutter: string
    readonly gap: string
    readonly gapGutter: string
    readonly buffer: string
  }
  readonly text: {
    readonly primary: string
    readonly secondary: string
    readonly muted: string
    readonly lineNumber: string
    readonly meta: string
    readonly addition: string
    readonly deletion: string
  }
  readonly border: {
    readonly default: string
    readonly subtle: string
    readonly rowDivider: string
  }
  readonly bufferStripe: string
  readonly syntax: GitReviewSyntaxColors
}

export interface FrontendTheme {
  readonly colors: {
    readonly text: {
      readonly primary: string
      readonly strong: string
      readonly secondary: string
      readonly muted: string
      readonly subtle: string
      readonly inverse: string
      readonly danger: string
      readonly accent: string
    }
    readonly icon: {
      readonly default: string
      readonly muted: string
      readonly subtle: string
      readonly accent: string
      readonly danger: string
      readonly success: string
    }
    readonly avatar: {
      readonly background: string
    }
    readonly surface: {
      readonly leftPanel: string
      readonly mainPanel: string
      readonly rightPanel: string
      readonly card: string
      readonly input: string
      readonly popover: string
      readonly muted: string
      readonly selected: string
      readonly selectedSubtle: string
      readonly infoSubtle: string
      readonly successSubtle: string
      readonly neutralSubtle: string
      readonly disabled: string
      readonly dangerSubtle: string
      readonly dangerSoft: string
      readonly errorSubtle: string
      readonly glass: string
      readonly glassStrong: string
      readonly glassInput: string
    }
    readonly border: {
      readonly hairline: string
      readonly subtle: string
      readonly default: string
      readonly strong: string
      readonly error: string
      readonly rightToolbarDivider: string
    }
    readonly button: {
      readonly primaryBg: string
      readonly primaryBgHover: string
      readonly primaryText: string
      readonly secondaryBg: string
      readonly secondaryBgHover: string
      readonly secondaryText: string
      readonly dangerBg: string
      readonly dangerBgHover: string
      readonly dangerText: string
      readonly dangerSoftText: string
      readonly dangerSoftBg: string
      readonly dangerSoftBgHover: string
    }
    readonly control: {
      readonly selectedBackground: string
      readonly selectedText: string
    }
    readonly sidebar: {
      readonly textSecondary: string
      readonly textActive: string
      readonly translucentTint: string
    }
    readonly settings: {
      readonly contentTitle: string
      readonly contentText: string
      readonly contentMuted: string
    }
    readonly state: {
      readonly hover: string
      readonly active: string
      readonly focusRing: string
      readonly resizeHandle: string
      readonly overlay: string
      readonly overlayLight: string
    }
    readonly status: {
      readonly successText: string
      readonly successSurface: string
    }
    readonly diff: {
      readonly additionText: string
      readonly deletionText: string
    }
    readonly gitReview: GitReviewColors
    readonly dataViz: {
      readonly input: string
      readonly output: string
      readonly outputThinking: string
    }
    readonly sourceBadge: {
      readonly text: string
      readonly background: string
    }
    readonly effect: {
      readonly shimmerHighlight: string
    }
    readonly terminal: {
      readonly background: string
      readonly foreground: string
      readonly cursor: string
      readonly selectionBackground: string
      readonly black: string
      readonly red: string
      readonly green: string
      readonly yellow: string
      readonly blue: string
      readonly magenta: string
      readonly cyan: string
      readonly white: string
      readonly brightBlack: string
      readonly brightRed: string
      readonly brightGreen: string
      readonly brightYellow: string
      readonly brightBlue: string
      readonly brightMagenta: string
      readonly brightCyan: string
      readonly brightWhite: string
    }
    readonly mediaViewer: {
      readonly foreground: string
      readonly backdrop: string
      readonly controlBackground: string
      readonly controlHover: string
      readonly focusRing: string
      readonly menuBackground: string
      readonly menuBorder: string
    }
  }
  readonly shadow: {
    readonly none: string
    readonly hairline: string
    readonly focus: string
    readonly composer: string
    readonly popover: string
    readonly popoverLarge: string
    readonly sidebarMenu: string
    readonly card: string
    readonly dialog: string
    readonly mediaViewerMenu: string
  }
}

export interface FrontendThemeDefinition {
  readonly colorScheme: ColorScheme
  readonly labelKey: TranslationKey
  readonly order: number
  readonly tokens: FrontendTheme
}
