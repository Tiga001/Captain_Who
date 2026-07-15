import { ayuDarkTheme, ayuLightTheme, ayuMirageTheme } from './ayu'
import { catppuccinLatteTheme, catppuccinMochaTheme } from './catppuccin'
import { classicDarkTheme, classicLightTheme } from './classic'
import { crabLightTheme } from './crab'
import { alucardLightTheme, draculaDarkTheme } from './dracula'
import { everforestDarkTheme, everforestLightTheme } from './everforest'
import { githubDarkTheme, githubLightTheme } from './github'
import { gruvboxDarkTheme, gruvboxLightTheme } from './gruvbox'
import { oneDarkTheme, oneLightTheme } from './one'
import { zjuLightTheme } from './zju'
import type { ColorScheme, ColorSchemePreference, FrontendThemeDefinition } from './types'

export const frontendThemes = {
  'classic-light': {
    colorScheme: 'light',
    labelKey: 'appearance.themeVariant.classicLight',
    order: 10,
    tokens: classicLightTheme
  },
  'classic-dark': {
    colorScheme: 'dark',
    labelKey: 'appearance.themeVariant.classicDark',
    order: 10,
    tokens: classicDarkTheme
  },
  'github-light': {
    colorScheme: 'light',
    labelKey: 'appearance.themeVariant.github',
    order: 20,
    tokens: githubLightTheme
  },
  'github-dark': {
    colorScheme: 'dark',
    labelKey: 'appearance.themeVariant.github',
    order: 20,
    tokens: githubDarkTheme
  },
  'catppuccin-latte-light': {
    colorScheme: 'light',
    labelKey: 'appearance.themeVariant.catppuccinLatte',
    order: 30,
    tokens: catppuccinLatteTheme
  },
  'catppuccin-mocha-dark': {
    colorScheme: 'dark',
    labelKey: 'appearance.themeVariant.catppuccinMocha',
    order: 30,
    tokens: catppuccinMochaTheme
  },
  'everforest-light': {
    colorScheme: 'light',
    labelKey: 'appearance.themeVariant.everforest',
    order: 40,
    tokens: everforestLightTheme
  },
  'everforest-dark': {
    colorScheme: 'dark',
    labelKey: 'appearance.themeVariant.everforest',
    order: 40,
    tokens: everforestDarkTheme
  },
  'ayu-light': {
    colorScheme: 'light',
    labelKey: 'appearance.themeVariant.ayuLight',
    order: 50,
    tokens: ayuLightTheme
  },
  'ayu-mirage-dark': {
    colorScheme: 'dark',
    labelKey: 'appearance.themeVariant.ayuMirage',
    order: 50,
    tokens: ayuMirageTheme
  },
  'ayu-dark': {
    colorScheme: 'dark',
    labelKey: 'appearance.themeVariant.ayuDark',
    order: 60,
    tokens: ayuDarkTheme
  },
  'alucard-light': {
    colorScheme: 'light',
    labelKey: 'appearance.themeVariant.alucard',
    order: 60,
    tokens: alucardLightTheme
  },
  'dracula-dark': {
    colorScheme: 'dark',
    labelKey: 'appearance.themeVariant.dracula',
    order: 70,
    tokens: draculaDarkTheme
  },
  'gruvbox-light': {
    colorScheme: 'light',
    labelKey: 'appearance.themeVariant.gruvbox',
    order: 70,
    tokens: gruvboxLightTheme
  },
  'gruvbox-dark': {
    colorScheme: 'dark',
    labelKey: 'appearance.themeVariant.gruvbox',
    order: 80,
    tokens: gruvboxDarkTheme
  },
  'one-light': {
    colorScheme: 'light',
    labelKey: 'appearance.themeVariant.one',
    order: 80,
    tokens: oneLightTheme
  },
  'one-dark': {
    colorScheme: 'dark',
    labelKey: 'appearance.themeVariant.one',
    order: 90,
    tokens: oneDarkTheme
  },
  'zju-light': {
    colorScheme: 'light',
    labelKey: 'appearance.themeVariant.zju',
    order: 90,
    tokens: zjuLightTheme
  },
  'crab-light': {
    colorScheme: 'light',
    labelKey: 'appearance.themeVariant.crab',
    order: 100,
    tokens: crabLightTheme
  }
} as const satisfies Record<string, FrontendThemeDefinition>

export type FrontendThemeId = keyof typeof frontendThemes

export type FrontendThemeIdForColorScheme<Scheme extends ColorScheme> = {
  [ThemeId in FrontendThemeId]: (typeof frontendThemes)[ThemeId]['colorScheme'] extends Scheme
    ? ThemeId
    : never
}[FrontendThemeId]

export interface RegisteredFrontendTheme extends FrontendThemeDefinition {
  readonly id: FrontendThemeId
}

export type ThemeIdsByColorScheme = {
  [Scheme in ColorScheme]: FrontendThemeIdForColorScheme<Scheme>
}

export const defaultThemeIdsByColorScheme = {
  light: 'classic-light',
  dark: 'classic-dark'
} as const satisfies ThemeIdsByColorScheme

const frontendThemeIds = Object.keys(frontendThemes) as FrontendThemeId[]

export function isColorScheme(value: unknown): value is ColorScheme {
  return value === 'light' || value === 'dark'
}

export function isColorSchemePreference(value: unknown): value is ColorSchemePreference {
  return value === 'system' || isColorScheme(value)
}

export function isFrontendThemeId(value: unknown): value is FrontendThemeId {
  return typeof value === 'string' && Object.hasOwn(frontendThemes, value)
}

export function getFrontendTheme(themeId: FrontendThemeId): RegisteredFrontendTheme {
  const definition = frontendThemes[themeId]
  return { id: themeId, ...definition }
}

export function getFrontendThemesForColorScheme(
  colorScheme: ColorScheme
): RegisteredFrontendTheme[] {
  return frontendThemeIds
    .map(getFrontendTheme)
    .filter((theme) => theme.colorScheme === colorScheme)
    .sort((left, right) => left.order - right.order || left.id.localeCompare(right.id))
}

export function isThemeIdForColorScheme<Scheme extends ColorScheme>(
  themeId: unknown,
  colorScheme: Scheme
): themeId is FrontendThemeIdForColorScheme<Scheme> {
  return isFrontendThemeId(themeId) && frontendThemes[themeId].colorScheme === colorScheme
}

export function getDefaultThemeIdForColorScheme<Scheme extends ColorScheme>(
  colorScheme: Scheme
): FrontendThemeIdForColorScheme<Scheme> {
  // The satisfies check above enforces this correlation; TypeScript loses it on generic indexing.
  return defaultThemeIdsByColorScheme[
    colorScheme
  ] as unknown as FrontendThemeIdForColorScheme<Scheme>
}
