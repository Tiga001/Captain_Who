import { createPaletteTheme } from './paletteTheme'
import type { PaletteThemeRecipe } from './paletteTheme'

const githubLightGitReviewSyntax = {
  attribute: '#0550AE',
  comment: '#6E7781',
  foreground: '#1F2328',
  function: '#8250DF',
  keyword: '#CF222E',
  number: '#0550AE',
  regexp: '#0A3069',
  string: '#0A3069',
  tag: '#116329',
  type: '#953800',
  variable: '#0550AE'
} as const

const githubDarkGitReviewSyntax = {
  attribute: '#79C0FF',
  comment: '#8B949E',
  foreground: '#E6EDF3',
  function: '#D2A8FF',
  keyword: '#FF7B72',
  number: '#79C0FF',
  regexp: '#A5D6FF',
  string: '#A5D6FF',
  tag: '#7EE787',
  type: '#FFA657',
  variable: '#79C0FF'
} as const

const githubLightRecipe = {
  colorScheme: 'light',
  text: {
    primary: '#1F2328',
    strong: '#1F2328',
    secondary: '#424A53',
    muted: '#59636E',
    subtle: '#818B98',
    inverse: '#FFFFFF'
  },
  surface: {
    leftPanel: '#F6F8FA',
    mainPanel: '#FFFFFF',
    rightPanel: '#FFFFFF',
    card: '#F6F8FA',
    input: '#F6F8FA',
    popover: '#FFFFFF',
    muted: '#F6F8FA',
    selected: '#EFF2F5',
    selectedSubtle: '#F6F8FA',
    disabled: '#EFF2F5',
    infoSubtle: '#DDF4FF',
    successSubtle: '#DAFBE1',
    dangerSubtle: '#FFEBE9'
  },
  border: {
    default: '#D1D9E0',
    strong: '#818B98'
  },
  accent: '#0969DA',
  semantic: {
    danger: '#D1242F',
    success: '#1A7F37',
    info: '#0969DA'
  },
  button: {
    primaryBg: '#1F883D',
    primaryBgHover: '#1C8139',
    primaryText: '#FFFFFF',
    dangerBg: '#CF222E',
    dangerBgHover: '#A40E26',
    dangerText: '#FFFFFF'
  },
  visual: {
    avatar: '#8250DF',
    input: '#0969DA',
    output: '#8250DF',
    outputThinking: '#BF3989',
    sourceBadgeBackground: '#0969DA',
    sourceBadgeText: '#FFFFFF'
  },
  gitReview: {
    syntax: githubLightGitReviewSyntax
  },
  terminal: {
    background: '#FFFFFF',
    foreground: '#1F2328',
    cursor: '#1F2328',
    selectionBackground: '#EFF2F5',
    black: '#1F2328',
    red: '#CF222E',
    green: '#116329',
    yellow: '#4D2D00',
    blue: '#0969DA',
    magenta: '#8250DF',
    cyan: '#1B7C83',
    white: '#59636E',
    brightBlack: '#393F46',
    brightRed: '#A40E26',
    brightGreen: '#1A7F37',
    brightYellow: '#633C01',
    brightBlue: '#218BFF',
    brightMagenta: '#A475F9',
    brightCyan: '#3192AA',
    brightWhite: '#818B98'
  },
  shadow: '#1F2328'
} as const satisfies PaletteThemeRecipe

const githubDarkRecipe = {
  colorScheme: 'dark',
  text: {
    primary: '#F0F6FC',
    strong: '#FFFFFF',
    secondary: '#C9D1D9',
    muted: '#9198A1',
    subtle: '#656C76',
    inverse: '#010409'
  },
  surface: {
    leftPanel: '#010409',
    mainPanel: '#0D1117',
    rightPanel: '#0D1117',
    card: '#151B23',
    input: '#151B23',
    popover: '#151B23',
    muted: '#151B23',
    selected: '#212830',
    selectedSubtle: '#1A2029',
    disabled: '#212830',
    infoSubtle: '#151F32',
    successSubtle: '#172B1D',
    dangerSubtle: '#2B181A'
  },
  border: {
    default: '#3D444D',
    strong: '#656C76'
  },
  accent: '#4493F8',
  semantic: {
    danger: '#F85149',
    success: '#3FB950',
    info: '#4493F8'
  },
  button: {
    primaryBg: '#238636',
    primaryBgHover: '#237A33',
    primaryText: '#FFFFFF',
    dangerBg: '#B62324',
    dangerBgHover: '#C93C37',
    dangerText: '#FFFFFF'
  },
  visual: {
    avatar: '#8957E5',
    input: '#58A6FF',
    output: '#BE8FFF',
    outputThinking: '#DB61A2',
    sourceBadgeBackground: '#1F6FEB',
    sourceBadgeText: '#FFFFFF'
  },
  gitReview: {
    syntax: githubDarkGitReviewSyntax
  },
  terminal: {
    background: '#0D1117',
    foreground: '#F0F6FC',
    cursor: '#F0F6FC',
    selectionBackground: '#264F78',
    black: '#2F3742',
    red: '#FF7B72',
    green: '#3FB950',
    yellow: '#D29922',
    blue: '#58A6FF',
    magenta: '#BE8FFF',
    cyan: '#39C5CF',
    white: '#F0F6FC',
    brightBlack: '#656C76',
    brightRed: '#FFA198',
    brightGreen: '#56D364',
    brightYellow: '#E3B341',
    brightBlue: '#79C0FF',
    brightMagenta: '#D2A8FF',
    brightCyan: '#56D4DD',
    brightWhite: '#FFFFFF'
  },
  shadow: '#010409'
} as const satisfies PaletteThemeRecipe

export const githubLightTheme = createPaletteTheme(githubLightRecipe)
export const githubDarkTheme = createPaletteTheme(githubDarkRecipe)
