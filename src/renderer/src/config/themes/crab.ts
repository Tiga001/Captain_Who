import { createPaletteTheme } from './paletteTheme'
import type { PaletteThemeRecipe } from './paletteTheme'

const crabGitReviewSyntax = {
  attribute: '#2B7071',
  comment: '#898278',
  constant: '#8A6500',
  foreground: '#1D1B16',
  function: '#8B4E66',
  keyword: '#B93D24',
  number: '#45656C',
  regexp: '#2B7071',
  string: '#007A35',
  tag: '#B93D24',
  type: '#45656C',
  variable: '#8A6500'
} as const

const crabPalette = {
  accent: '#DA7756',
  accentStrong: '#A64B31',
  ink: '#1D1B16',
  paper: '#F5F3EE',
  skill: '#CC7D5E'
} as const

const crabLightRecipe = {
  colorScheme: 'light',
  text: {
    primary: '#3A3731',
    strong: crabPalette.ink,
    secondary: '#514D45',
    muted: '#6E695F',
    subtle: '#898278',
    inverse: '#FFFFFF'
  },
  surface: {
    leftPanel: '#ECE8E0',
    mainPanel: crabPalette.paper,
    rightPanel: '#F9F7F2',
    card: '#FFFDF8',
    input: '#F9F6F0',
    popover: '#FFFDF8',
    muted: '#EEEAE3',
    selected: '#EAD8CE',
    selectedSubtle: '#F1E5DF',
    disabled: '#E1DDD4',
    infoSubtle: '#E9F0F0',
    successSubtle: '#E5F2E8',
    neutralSubtle: '#EEEAE3',
    dangerSubtle: '#FAE8E2'
  },
  border: {
    default: '#D7D1C6',
    strong: '#B7ADA0'
  },
  accent: crabPalette.accentStrong,
  semantic: {
    danger: '#B93D24',
    success: '#007A35',
    info: '#45656C'
  },
  button: {
    primaryBg: crabPalette.accent,
    primaryBgHover: '#C96A4C',
    primaryText: crabPalette.ink,
    dangerBg: '#B93D24',
    dangerBgHover: '#A83A27',
    dangerText: '#FFFFFF'
  },
  visual: {
    avatar: crabPalette.accent,
    input: crabPalette.accentStrong,
    output: crabPalette.skill,
    outputThinking: '#8B5E55',
    sourceBadgeBackground: crabPalette.accentStrong,
    sourceBadgeText: '#FFFFFF'
  },
  gitReview: {
    syntax: crabGitReviewSyntax
  },
  terminal: {
    background: crabPalette.paper,
    foreground: crabPalette.ink,
    cursor: crabPalette.accentStrong,
    selectionBackground: '#EAD8CE',
    black: crabPalette.ink,
    red: '#B93D24',
    green: '#007A35',
    yellow: '#8A6500',
    blue: '#45656C',
    magenta: '#8B4E66',
    cyan: '#2B7071',
    white: '#898278',
    brightBlack: '#6E695F',
    brightRed: '#FF5F38',
    brightGreen: '#00C853',
    brightYellow: '#B47A00',
    brightBlue: '#5B7E86',
    brightMagenta: '#A86178',
    brightCyan: '#3E8888',
    brightWhite: '#3A3731'
  },
  shadow: '#3A2F29'
} as const satisfies PaletteThemeRecipe

export const crabLightTheme = createPaletteTheme(crabLightRecipe)
