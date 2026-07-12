import type { ColorScheme, FrontendTheme } from './types'

interface PaletteThemeSurface {
  readonly leftPanel: string
  readonly mainPanel: string
  readonly rightPanel: string
  readonly card: string
  readonly input: string
  readonly popover: string
  readonly muted: string
  readonly selected: string
  readonly selectedSubtle: string
  readonly disabled: string
  readonly infoSubtle?: string
  readonly successSubtle?: string
  readonly neutralSubtle?: string
  readonly dangerSubtle?: string
}

export interface PaletteThemeRecipe {
  readonly colorScheme: ColorScheme
  readonly text: {
    readonly primary: string
    readonly strong: string
    readonly secondary: string
    readonly muted: string
    readonly subtle: string
    readonly inverse: string
  }
  readonly surface: PaletteThemeSurface
  readonly border: {
    readonly default: string
    readonly strong: string
  }
  readonly accent: string
  readonly semantic: {
    readonly danger: string
    readonly success: string
    readonly info: string
  }
  readonly button: {
    readonly primaryBg: string
    readonly primaryBgHover: string
    readonly primaryText: string
    readonly dangerBg: string
    readonly dangerBgHover: string
    readonly dangerText: string
  }
  readonly visual: {
    readonly avatar: string
    readonly input: string
    readonly output: string
    readonly outputThinking: string
    readonly sourceBadgeBackground: string
    readonly sourceBadgeText: string
  }
  readonly terminal: FrontendTheme['colors']['terminal']
  readonly shadow: string
}

function withAlpha(color: string, opacity: number): string {
  const match = /^#([0-9a-f]{6})$/i.exec(color)
  if (!match) throw new Error(`Palette theme colors must use six-digit hex values: ${color}`)

  const value = Number.parseInt(match[1], 16)
  const red = (value >> 16) & 255
  const green = (value >> 8) & 255
  const blue = value & 255
  return `rgba(${red}, ${green}, ${blue}, ${opacity})`
}

export function createPaletteTheme(recipe: PaletteThemeRecipe): FrontendTheme {
  const isDark = recipe.colorScheme === 'dark'
  const { accent, border, button, semantic, shadow, surface, text, visual } = recipe
  const secondaryButtonBg = isDark ? surface.muted : surface.input
  const secondaryButtonHover = isDark ? surface.selected : surface.muted

  return {
    colors: {
      text: {
        ...text,
        danger: semantic.danger,
        accent
      },
      icon: {
        default: text.primary,
        muted: text.secondary,
        subtle: text.subtle,
        accent,
        danger: semantic.danger,
        success: semantic.success
      },
      avatar: {
        background: visual.avatar
      },
      surface: {
        leftPanel: surface.leftPanel,
        mainPanel: surface.mainPanel,
        rightPanel: surface.rightPanel,
        card: surface.card,
        input: surface.input,
        popover: surface.popover,
        muted: surface.muted,
        selected: surface.selected,
        selectedSubtle: surface.selectedSubtle,
        infoSubtle: surface.infoSubtle ?? withAlpha(semantic.info, isDark ? 0.14 : 0.1),
        successSubtle: surface.successSubtle ?? withAlpha(semantic.success, isDark ? 0.14 : 0.1),
        neutralSubtle: surface.neutralSubtle ?? surface.muted,
        disabled: surface.disabled,
        dangerSubtle: surface.dangerSubtle ?? withAlpha(semantic.danger, isDark ? 0.14 : 0.1),
        dangerSoft: withAlpha(semantic.danger, isDark ? 0.14 : 0.08),
        errorSubtle: withAlpha(semantic.danger, isDark ? 0.13 : 0.08),
        glass: withAlpha(surface.popover, isDark ? 0.92 : 0.94),
        glassStrong: withAlpha(surface.popover, 0.97),
        glassInput: isDark ? withAlpha(text.primary, 0.08) : withAlpha(surface.input, 0.82)
      },
      border: {
        hairline: withAlpha(text.primary, isDark ? 0.1 : 0.08),
        subtle: withAlpha(border.default, 0.76),
        default: border.default,
        strong: border.strong,
        error: withAlpha(semantic.danger, isDark ? 0.28 : 0.22),
        rightToolbarDivider: withAlpha(text.primary, isDark ? 0.07 : 0.06)
      },
      button: {
        primaryBg: button.primaryBg,
        primaryBgHover: button.primaryBgHover,
        primaryText: button.primaryText,
        secondaryBg: secondaryButtonBg,
        secondaryBgHover: secondaryButtonHover,
        secondaryText: text.primary,
        dangerBg: button.dangerBg,
        dangerBgHover: button.dangerBgHover,
        dangerText: button.dangerText,
        dangerSoftText: semantic.danger,
        dangerSoftBg: withAlpha(semantic.danger, isDark ? 0.14 : 0.09),
        dangerSoftBgHover: withAlpha(semantic.danger, isDark ? 0.22 : 0.14)
      },
      sidebar: {
        textSecondary: text.secondary,
        textActive: text.strong,
        translucentTint: surface.leftPanel
      },
      settings: {
        contentTitle: text.strong,
        contentText: text.primary,
        contentMuted: text.muted
      },
      state: {
        hover: withAlpha(text.primary, isDark ? 0.08 : 0.055),
        active: withAlpha(accent, isDark ? 0.17 : 0.11),
        focusRing: withAlpha(accent, isDark ? 0.82 : 0.52),
        resizeHandle: withAlpha(accent, isDark ? 0.86 : 0.58),
        overlay: withAlpha(shadow, isDark ? 0.58 : 0.22),
        overlayLight: isDark ? withAlpha(shadow, 0.48) : withAlpha(surface.mainPanel, 0.5)
      },
      status: {
        successText: semantic.success,
        successSurface: withAlpha(semantic.success, isDark ? 0.14 : 0.1)
      },
      diff: {
        additionText: semantic.success,
        deletionText: semantic.danger
      },
      dataViz: {
        input: visual.input,
        output: visual.output,
        outputThinking: visual.outputThinking
      },
      sourceBadge: {
        text: visual.sourceBadgeText,
        background: visual.sourceBadgeBackground
      },
      effect: {
        shimmerHighlight: withAlpha('#FFFFFF', isDark ? 0.7 : 0.76)
      },
      terminal: recipe.terminal,
      mediaViewer: {
        foreground: withAlpha('#FFFFFF', 0.95),
        backdrop: withAlpha(shadow, 0.9),
        controlBackground: withAlpha(shadow, 0.94),
        controlHover: withAlpha(text.primary, 0.96),
        focusRing: withAlpha(accent, 0.86),
        menuBackground: withAlpha(shadow, 0.98),
        menuBorder: withAlpha('#FFFFFF', 0.14)
      }
    },
    shadow: {
      none: 'none',
      hairline: `0 1px 2px ${withAlpha(shadow, isDark ? 0.34 : 0.04)}`,
      focus: `0 0 0 3px ${withAlpha(accent, isDark ? 0.24 : 0.14)}`,
      composer: isDark
        ? `0 18px 46px ${withAlpha(shadow, 0.48)}, 0 2px 8px ${withAlpha(shadow, 0.34)}`
        : `0 16px 42px ${withAlpha(shadow, 0.12)}, 0 2px 8px ${withAlpha(shadow, 0.05)}`,
      popover: isDark
        ? `0 18px 48px ${withAlpha(shadow, 0.58)}, 0 3px 10px ${withAlpha(shadow, 0.4)}`
        : `0 16px 44px ${withAlpha(shadow, 0.16)}, 0 3px 10px ${withAlpha(shadow, 0.07)}`,
      popoverLarge: isDark
        ? `0 22px 58px ${withAlpha(shadow, 0.64)}, 0 6px 18px ${withAlpha(shadow, 0.44)}`
        : `0 22px 56px ${withAlpha(shadow, 0.18)}, 0 6px 18px ${withAlpha(shadow, 0.09)}`,
      sidebarMenu: isDark
        ? `0 22px 48px ${withAlpha(shadow, 0.6)}, 0 3px 10px ${withAlpha(shadow, 0.42)}`
        : `0 20px 46px ${withAlpha(shadow, 0.16)}, 0 3px 10px ${withAlpha(shadow, 0.08)}`,
      card: `0 10px 28px ${withAlpha(shadow, isDark ? 0.44 : 0.09)}`,
      dialog: isDark
        ? `0 32px 72px ${withAlpha(shadow, 0.7)}, 0 8px 24px ${withAlpha(shadow, 0.52)}`
        : `0 30px 68px ${withAlpha(shadow, 0.24)}, 0 8px 24px ${withAlpha(shadow, 0.12)}`,
      mediaViewerMenu: `0 16px 42px ${withAlpha(shadow, isDark ? 0.58 : 0.42)}`
    }
  }
}
