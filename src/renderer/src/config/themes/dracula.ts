import { createPaletteTheme } from './paletteTheme'
import type { PaletteThemeRecipe } from './paletteTheme'

const alucardGitReviewSyntax = {
  attribute: '#036A96',
  comment: '#6C664B',
  constant: '#846E15',
  foreground: '#1F1F1F',
  function: '#644AC9',
  keyword: '#CB3A2A',
  number: '#036A96',
  regexp: '#036A96',
  string: '#14710A',
  tag: '#CB3A2A',
  type: '#644AC9',
  variable: '#846E15'
} as const

const draculaGitReviewSyntax = {
  attribute: '#50FA7B',
  comment: '#6272A4',
  foreground: '#F8F8F2',
  function: '#50FA7B',
  keyword: '#FF79C6',
  number: '#BD93F9',
  regexp: '#FF5555',
  string: '#F1FA8C',
  tag: '#FF79C6',
  type: '#8BE9FD',
  variable: '#F8F8F2'
} as const

const alucardUiPalette = {
  background: '#FFFBEB',
  floating: '#EFEDDC',
  backgroundLighter: '#ECE9DF',
  backgroundLight: '#DEDCCF',
  backgroundDark: '#CECCC0',
  backgroundDarker: '#BCBAB3',
  selection: '#CFCFDE'
} as const

const alucardFunctionalPalette = {
  focus: '#815CD6'
} as const

const alucardRecipe = {
  colorScheme: 'light',
  text: {
    primary: '#1F1F1F',
    strong: '#1F1F1F',
    secondary: '#6C664B',
    muted: '#6C664B',
    subtle: '#6C664B',
    inverse: alucardUiPalette.background
  },
  surface: {
    leftPanel: alucardUiPalette.backgroundLight,
    mainPanel: alucardUiPalette.background,
    rightPanel: alucardUiPalette.backgroundLighter,
    card: alucardUiPalette.backgroundLighter,
    input: alucardUiPalette.floating,
    popover: alucardUiPalette.floating,
    muted: alucardUiPalette.backgroundLight,
    selected: alucardUiPalette.selection,
    selectedSubtle: alucardUiPalette.backgroundLight,
    disabled: alucardUiPalette.backgroundDark
  },
  border: {
    default: alucardUiPalette.backgroundDark,
    strong: alucardUiPalette.backgroundDarker
  },
  accent: alucardFunctionalPalette.focus,
  semantic: {
    danger: '#CB3A2A',
    success: '#14710A',
    info: '#036A96'
  },
  button: {
    primaryBg: alucardFunctionalPalette.focus,
    primaryBgHover: '#644AC9',
    primaryText: alucardUiPalette.background,
    dangerBg: '#B33124',
    dangerBgHover: '#93291F',
    dangerText: alucardUiPalette.background
  },
  visual: {
    avatar: '#A3144D',
    input: '#036A96',
    output: '#644AC9',
    outputThinking: '#A3144D',
    sourceBadgeBackground: '#036A96',
    sourceBadgeText: alucardUiPalette.background
  },
  gitReview: {
    syntax: alucardGitReviewSyntax
  },
  terminal: {
    background: alucardUiPalette.background,
    foreground: '#1F1F1F',
    cursor: '#1F1F1F',
    selectionBackground: alucardUiPalette.selection,
    black: alucardUiPalette.background,
    red: '#CB3A2A',
    green: '#14710A',
    yellow: '#846E15',
    blue: '#644AC9',
    magenta: '#A3144D',
    cyan: '#036A96',
    white: '#1F1F1F',
    brightBlack: '#6C664B',
    brightRed: '#D74C3D',
    brightGreen: '#198D0C',
    brightYellow: '#9E841A',
    brightBlue: '#7862D0',
    brightMagenta: '#BF185A',
    brightCyan: '#047FB4',
    brightWhite: '#2C2B31'
  },
  shadow: '#1F1F1F'
} as const satisfies PaletteThemeRecipe

const draculaRecipe = {
  colorScheme: 'dark',
  text: {
    primary: '#F8F8F2',
    strong: '#FFFFFF',
    secondary: '#D7D9E5',
    muted: '#A7ACC4',
    subtle: '#7F86A3',
    inverse: '#191A21'
  },
  surface: {
    leftPanel: '#21222C',
    mainPanel: '#282A36',
    rightPanel: '#242631',
    card: '#343746',
    input: '#343746',
    popover: '#343746',
    muted: '#343746',
    selected: '#44475A',
    selectedSubtle: '#353747',
    disabled: '#424450'
  },
  border: {
    default: '#44475A',
    strong: '#6272A4'
  },
  accent: '#BD93F9',
  semantic: {
    danger: '#FF5555',
    success: '#50FA7B',
    info: '#8BE9FD'
  },
  button: {
    primaryBg: '#BD93F9',
    primaryBgHover: '#CAA9FA',
    primaryText: '#191A21',
    dangerBg: '#C43C48',
    dangerBgHover: '#C9434E',
    dangerText: '#FFFFFF'
  },
  visual: {
    avatar: '#FF79C6',
    input: '#8BE9FD',
    output: '#BD93F9',
    outputThinking: '#FF79C6',
    sourceBadgeBackground: '#8BE9FD',
    sourceBadgeText: '#191A21'
  },
  gitReview: {
    syntax: draculaGitReviewSyntax
  },
  terminal: {
    background: '#282A36',
    foreground: '#F8F8F2',
    cursor: '#F8F8F2',
    selectionBackground: '#44475A',
    black: '#21222C',
    red: '#FF5555',
    green: '#50FA7B',
    yellow: '#F1FA8C',
    blue: '#BD93F9',
    magenta: '#FF79C6',
    cyan: '#8BE9FD',
    white: '#F8F8F2',
    brightBlack: '#6272A4',
    brightRed: '#FF6E6E',
    brightGreen: '#69FF94',
    brightYellow: '#FFFFA5',
    brightBlue: '#D6ACFF',
    brightMagenta: '#FF92DF',
    brightCyan: '#A4FFFF',
    brightWhite: '#FFFFFF'
  },
  shadow: '#191A21'
} as const satisfies PaletteThemeRecipe

export const alucardLightTheme = createPaletteTheme(alucardRecipe)
export const draculaDarkTheme = createPaletteTheme(draculaRecipe)
