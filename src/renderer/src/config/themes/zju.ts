import { createPaletteTheme } from './paletteTheme'
import type { PaletteThemeRecipe } from './paletteTheme'

const zjuGitReviewSyntax = {
  attribute: '#005AA7',
  comment: '#7D90A2',
  constant: '#8A5B00',
  foreground: '#263B50',
  function: '#743B68',
  keyword: '#B01F24',
  number: '#005AA7',
  regexp: '#006B80',
  string: '#176B64',
  tag: '#B01F24',
  type: '#003F88',
  variable: '#8A5B00'
} as const

const zjuBrandColors = {
  qushiBlue: '#003F88',
  innovationRed: '#B01F24'
} as const

const zjuLightRecipe = {
  colorScheme: 'light',
  text: {
    primary: '#263B50',
    strong: '#102A43',
    secondary: '#456078',
    muted: '#5F768C',
    subtle: '#7D90A2',
    inverse: '#FFFFFF'
  },
  surface: {
    leftPanel: '#EDF4FA',
    mainPanel: '#FFFFFF',
    rightPanel: '#F6F9FC',
    card: '#FFFFFF',
    input: '#F2F6FA',
    popover: '#FFFFFF',
    muted: '#ECF2F7',
    selected: '#DCEAF6',
    selectedSubtle: '#EDF4FA',
    disabled: '#E2EAF1',
    infoSubtle: '#E8F2FC',
    successSubtle: '#E8F4F2',
    neutralSubtle: '#ECF2F7',
    dangerSubtle: '#FBECEE'
  },
  border: {
    default: '#C5D5E4',
    strong: '#7299BF'
  },
  accent: zjuBrandColors.qushiBlue,
  semantic: {
    danger: zjuBrandColors.innovationRed,
    success: '#176B64',
    info: '#005AA7'
  },
  button: {
    primaryBg: zjuBrandColors.qushiBlue,
    primaryBgHover: '#002F66',
    primaryText: '#FFFFFF',
    dangerBg: zjuBrandColors.innovationRed,
    dangerBgHover: '#8F171C',
    dangerText: '#FFFFFF'
  },
  visual: {
    avatar: zjuBrandColors.innovationRed,
    input: zjuBrandColors.qushiBlue,
    output: zjuBrandColors.innovationRed,
    outputThinking: '#743B68',
    sourceBadgeBackground: zjuBrandColors.qushiBlue,
    sourceBadgeText: '#FFFFFF'
  },
  gitReview: {
    syntax: zjuGitReviewSyntax
  },
  terminal: {
    background: '#FFFFFF',
    foreground: '#263B50',
    cursor: zjuBrandColors.qushiBlue,
    selectionBackground: '#DCEAF6',
    black: '#102A43',
    red: zjuBrandColors.innovationRed,
    green: '#176B64',
    yellow: '#8A5B00',
    blue: zjuBrandColors.qushiBlue,
    magenta: '#743B68',
    cyan: '#006B80',
    white: '#5F768C',
    brightBlack: '#7D90A2',
    brightRed: '#C43A3F',
    brightGreen: '#24827A',
    brightYellow: '#A46D00',
    brightBlue: '#005AA7',
    brightMagenta: '#935381',
    brightCyan: '#00849C',
    brightWhite: '#263B50'
  },
  shadow: '#12385F'
} as const satisfies PaletteThemeRecipe

export const zjuLightTheme = createPaletteTheme(zjuLightRecipe)
