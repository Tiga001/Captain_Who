import { createPaletteTheme } from './paletteTheme'
import type { PaletteThemeRecipe } from './paletteTheme'

const catppuccinLatteGitReviewSyntax = {
  attribute: '#DF8E1D',
  comment: '#7C7F93',
  foreground: '#4C4F69',
  function: '#1E66F5',
  keyword: '#8839EF',
  number: '#FE640B',
  regexp: '#EA76CB',
  string: '#40A02B',
  tag: '#1E66F5',
  type: '#DF8E1D',
  variable: '#4C4F69'
} as const

const catppuccinMochaGitReviewSyntax = {
  attribute: '#F9E2AF',
  comment: '#9399B2',
  foreground: '#CDD6F4',
  function: '#89B4FA',
  keyword: '#CBA6F7',
  number: '#FAB387',
  regexp: '#F5C2E7',
  string: '#A6E3A1',
  tag: '#89B4FA',
  type: '#F9E2AF',
  variable: '#CDD6F4'
} as const

const catppuccinLatteRecipe = {
  colorScheme: 'light',
  text: {
    primary: '#4C4F69',
    strong: '#3C3F57',
    secondary: '#5C5F77',
    muted: '#6C6F85',
    subtle: '#7C7F93',
    inverse: '#EFF1F5'
  },
  surface: {
    leftPanel: '#E6E9EF',
    mainPanel: '#EFF1F5',
    rightPanel: '#EFF1F5',
    card: '#F4F5F9',
    input: '#F4F5F9',
    popover: '#F7F8FB',
    muted: '#E6E9EF',
    selected: '#CCD0DA',
    selectedSubtle: '#DCE0E8',
    disabled: '#CCD0DA'
  },
  border: {
    default: '#BCC0CC',
    strong: '#9CA0B0'
  },
  accent: '#1A5BD7',
  semantic: {
    danger: '#D20F39',
    success: '#2F7D22',
    info: '#1E66F5'
  },
  button: {
    primaryBg: '#1E66F5',
    primaryBgHover: '#174FC0',
    primaryText: '#FFFFFF',
    dangerBg: '#B80D32',
    dangerBgHover: '#9F0B2B',
    dangerText: '#FFFFFF'
  },
  visual: {
    avatar: '#8839EF',
    input: '#1E66F5',
    output: '#8839EF',
    outputThinking: '#A3146D',
    sourceBadgeBackground: '#1E66F5',
    sourceBadgeText: '#FFFFFF'
  },
  gitReview: {
    syntax: catppuccinLatteGitReviewSyntax
  },
  terminal: {
    background: '#EFF1F5',
    foreground: '#4C4F69',
    cursor: '#4C4F69',
    selectionBackground: '#CCD0DA',
    black: '#5C5F77',
    red: '#D20F39',
    green: '#40A02B',
    yellow: '#DF8E1D',
    blue: '#1E66F5',
    magenta: '#EA76CB',
    cyan: '#179299',
    white: '#ACB0BE',
    brightBlack: '#6C6F85',
    brightRed: '#DE293E',
    brightGreen: '#49AF3D',
    brightYellow: '#EEA02D',
    brightBlue: '#456EFF',
    brightMagenta: '#FE85D8',
    brightCyan: '#2D9FA8',
    brightWhite: '#BCC0CC'
  },
  shadow: '#4C4F69'
} as const satisfies PaletteThemeRecipe

const catppuccinMochaRecipe = {
  colorScheme: 'dark',
  text: {
    primary: '#CDD6F4',
    strong: '#E6E9FF',
    secondary: '#BAC2DE',
    muted: '#A6ADC8',
    subtle: '#7F849C',
    inverse: '#11111B'
  },
  surface: {
    leftPanel: '#181825',
    mainPanel: '#1E1E2E',
    rightPanel: '#1B1B29',
    card: '#252538',
    input: '#252538',
    popover: '#252538',
    muted: '#313244',
    selected: '#45475A',
    selectedSubtle: '#383A4F',
    disabled: '#45475A'
  },
  border: {
    default: '#45475A',
    strong: '#585B70'
  },
  accent: '#B4BEFE',
  semantic: {
    danger: '#F38BA8',
    success: '#A6E3A1',
    info: '#89B4FA'
  },
  button: {
    primaryBg: '#89B4FA',
    primaryBgHover: '#B4BEFE',
    primaryText: '#11111B',
    dangerBg: '#F38BA8',
    dangerBgHover: '#F5A0B8',
    dangerText: '#11111B'
  },
  visual: {
    avatar: '#CBA6F7',
    input: '#89B4FA',
    output: '#CBA6F7',
    outputThinking: '#F5C2E7',
    sourceBadgeBackground: '#74C7EC',
    sourceBadgeText: '#11111B'
  },
  gitReview: {
    syntax: catppuccinMochaGitReviewSyntax
  },
  terminal: {
    background: '#1E1E2E',
    foreground: '#CDD6F4',
    cursor: '#B4BEFE',
    selectionBackground: '#45475A',
    black: '#45475A',
    red: '#F38BA8',
    green: '#A6E3A1',
    yellow: '#F9E2AF',
    blue: '#89B4FA',
    magenta: '#F5C2E7',
    cyan: '#94E2D5',
    white: '#A6ADC8',
    brightBlack: '#585B70',
    brightRed: '#F37799',
    brightGreen: '#89D88B',
    brightYellow: '#EBD391',
    brightBlue: '#74A8FC',
    brightMagenta: '#F2AEDE',
    brightCyan: '#6BD7CA',
    brightWhite: '#BAC2DE'
  },
  shadow: '#11111B'
} as const satisfies PaletteThemeRecipe

export const catppuccinLatteTheme = createPaletteTheme(catppuccinLatteRecipe)
export const catppuccinMochaTheme = createPaletteTheme(catppuccinMochaRecipe)
