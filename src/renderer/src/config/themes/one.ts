import { createPaletteTheme } from './paletteTheme'
import type { PaletteThemeRecipe } from './paletteTheme'

const oneLightGitReviewSyntax = {
  attribute: '#986801',
  comment: '#A0A1A7',
  foreground: '#383A42',
  function: '#4078F2',
  keyword: '#A626A4',
  number: '#986801',
  regexp: '#0184BC',
  string: '#50A14F',
  tag: '#E45649',
  type: '#C18401',
  variable: '#986801'
} as const

const oneDarkGitReviewSyntax = {
  attribute: '#D19A66',
  comment: '#7F848E',
  foreground: '#ABB2BF',
  function: '#61AFEF',
  keyword: '#C678DD',
  number: '#D19A66',
  regexp: '#E06C75',
  string: '#98C379',
  tag: '#E06C75',
  type: '#E5C07B',
  variable: '#E5C07B'
} as const

const oneLightRecipe = {
  colorScheme: 'light',
  text: {
    primary: '#383A42',
    strong: '#2B2D33',
    secondary: '#51545E',
    muted: '#686B77',
    subtle: '#878993',
    inverse: '#FAFAFA'
  },
  surface: {
    leftPanel: '#F0F0F1',
    mainPanel: '#FAFAFA',
    rightPanel: '#F7F7F8',
    card: '#F7F7F8',
    input: '#F7F7F8',
    popover: '#FFFFFF',
    muted: '#F0F0F1',
    selected: '#DDE6F7',
    selectedSubtle: '#EAEDF3',
    disabled: '#E1E2E5'
  },
  border: {
    default: '#D7D8DC',
    strong: '#A0A1A7'
  },
  accent: '#315FCA',
  semantic: {
    danger: '#B4333A',
    success: '#3A7A39',
    info: '#315FCA'
  },
  button: {
    primaryBg: '#315FCA',
    primaryBgHover: '#284FA9',
    primaryText: '#FFFFFF',
    dangerBg: '#B4333A',
    dangerBgHover: '#932A30',
    dangerText: '#FFFFFF'
  },
  visual: {
    avatar: '#A626A4',
    input: '#315FCA',
    output: '#A626A4',
    outputThinking: '#CA1243',
    sourceBadgeBackground: '#315FCA',
    sourceBadgeText: '#FFFFFF'
  },
  gitReview: {
    syntax: oneLightGitReviewSyntax
  },
  terminal: {
    background: '#FAFAFA',
    foreground: '#383A42',
    cursor: '#383A42',
    selectionBackground: '#DDE6F7',
    black: '#383A42',
    red: '#E45649',
    green: '#50A14F',
    yellow: '#C18401',
    blue: '#4078F2',
    magenta: '#A626A4',
    cyan: '#0184BC',
    white: '#A0A1A7',
    brightBlack: '#686B77',
    brightRed: '#CA1243',
    brightGreen: '#3A7A39',
    brightYellow: '#986801',
    brightBlue: '#315FCA',
    brightMagenta: '#8A2188',
    brightCyan: '#01739F',
    brightWhite: '#383A42'
  },
  shadow: '#383A42'
} as const satisfies PaletteThemeRecipe

const oneDarkRecipe = {
  colorScheme: 'dark',
  text: {
    primary: '#ABB2BF',
    strong: '#D7DAE0',
    secondary: '#9DA5B4',
    muted: '#828997',
    subtle: '#6F7785',
    inverse: '#282C34'
  },
  surface: {
    leftPanel: '#21252B',
    mainPanel: '#282C34',
    rightPanel: '#252A31',
    card: '#2F3540',
    input: '#2F3540',
    popover: '#2F3540',
    muted: '#2C313A',
    selected: '#3A404B',
    selectedSubtle: '#303641',
    disabled: '#4B5363'
  },
  border: {
    default: '#3E4451',
    strong: '#5C6370'
  },
  accent: '#61AFEF',
  semantic: {
    danger: '#E4767E',
    success: '#98C379',
    info: '#61AFEF'
  },
  button: {
    primaryBg: '#61AFEF',
    primaryBgHover: '#79BDF2',
    primaryText: '#21252B',
    dangerBg: '#E06C75',
    dangerBgHover: '#E98289',
    dangerText: '#21252B'
  },
  visual: {
    avatar: '#C678DD',
    input: '#61AFEF',
    output: '#C678DD',
    outputThinking: '#D19A66',
    sourceBadgeBackground: '#61AFEF',
    sourceBadgeText: '#21252B'
  },
  gitReview: {
    syntax: oneDarkGitReviewSyntax
  },
  terminal: {
    background: '#282C34',
    foreground: '#ABB2BF',
    cursor: '#ABB2BF',
    selectionBackground: '#3A404B',
    black: '#282C34',
    red: '#E06C75',
    green: '#98C379',
    yellow: '#E5C07B',
    blue: '#61AFEF',
    magenta: '#C678DD',
    cyan: '#56B6C2',
    white: '#ABB2BF',
    brightBlack: '#5C6370',
    brightRed: '#E06C75',
    brightGreen: '#98C379',
    brightYellow: '#E5C07B',
    brightBlue: '#61AFEF',
    brightMagenta: '#C678DD',
    brightCyan: '#56B6C2',
    brightWhite: '#D7DAE0'
  },
  shadow: '#181A1F'
} as const satisfies PaletteThemeRecipe

export const oneLightTheme = createPaletteTheme(oneLightRecipe)
export const oneDarkTheme = createPaletteTheme(oneDarkRecipe)
