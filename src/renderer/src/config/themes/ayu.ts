import { createPaletteTheme } from './paletteTheme'
import type { PaletteThemeRecipe } from './paletteTheme'

const ayuLightGitReviewSyntax = {
  attribute: '#EBA400',
  comment: '#ADAEB1',
  foreground: '#5C6166',
  function: '#EBA400',
  keyword: '#FA8532',
  number: '#A37ACC',
  regexp: '#4CBF99',
  string: '#86B300',
  tag: '#55B4D4',
  type: '#22A4E6',
  variable: '#5C6166'
} as const

const ayuMirageGitReviewSyntax = {
  attribute: '#FFCD66',
  comment: '#6E7C8F',
  foreground: '#CCCAC2',
  function: '#FFCD66',
  keyword: '#FFA659',
  number: '#DFBFFF',
  regexp: '#95E6CB',
  string: '#D5FF80',
  tag: '#5CCFE6',
  type: '#73D0FF',
  variable: '#CCCAC2'
} as const

const ayuDarkGitReviewSyntax = {
  attribute: '#FFB454',
  comment: '#5A6673',
  foreground: '#BFBDB6',
  function: '#FFB454',
  keyword: '#FF8F40',
  number: '#D2A6FF',
  regexp: '#95E6CB',
  string: '#AAD94C',
  tag: '#39BAE6',
  type: '#59C2FF',
  variable: '#BFBDB6'
} as const

const ayuLightRecipe = {
  colorScheme: 'light',
  text: {
    primary: '#5C6166',
    strong: '#3B4045',
    secondary: '#68717D',
    muted: '#737D89',
    subtle: '#828E9F',
    inverse: '#FCFCFC'
  },
  surface: {
    leftPanel: '#F8F9FA',
    mainPanel: '#FCFCFC',
    rightPanel: '#FAFAFA',
    card: '#F8F9FA',
    input: '#F8F9FA',
    popover: '#FFFFFF',
    muted: '#F8F9FA',
    selected: '#DDE8F8',
    selectedSubtle: '#EBEEF0',
    disabled: '#E3E7EA'
  },
  border: {
    default: '#D7DDE3',
    strong: '#ADB6C1'
  },
  accent: '#006F9E',
  semantic: {
    danger: '#C63F3F',
    success: '#4D7600',
    info: '#006F9E'
  },
  button: {
    primaryBg: '#F29718',
    primaryBgHover: '#E18400',
    primaryText: '#3B2A00',
    dangerBg: '#B93D3D',
    dangerBgHover: '#9F3333',
    dangerText: '#FFFFFF'
  },
  visual: {
    avatar: '#F29718',
    input: '#006F9E',
    output: '#7B54A5',
    outputThinking: '#B55280',
    sourceBadgeBackground: '#006F9E',
    sourceBadgeText: '#FFFFFF'
  },
  gitReview: {
    syntax: ayuLightGitReviewSyntax
  },
  terminal: {
    background: '#FCFCFC',
    foreground: '#5C6166',
    cursor: '#5C6166',
    selectionBackground: '#DDE8F8',
    black: '#5C6166',
    red: '#F07171',
    green: '#86B300',
    yellow: '#EBA400',
    blue: '#22A4E6',
    magenta: '#A37ACC',
    cyan: '#4CBF99',
    white: '#ADAEB1',
    brightBlack: '#828E9F',
    brightRed: '#F07171',
    brightGreen: '#86B300',
    brightYellow: '#EBA400',
    brightBlue: '#22A4E6',
    brightMagenta: '#A37ACC',
    brightCyan: '#4CBF99',
    brightWhite: '#5C6166'
  },
  shadow: '#5C6166'
} as const satisfies PaletteThemeRecipe

const ayuMirageRecipe = {
  colorScheme: 'dark',
  text: {
    primary: '#CCCAC2',
    strong: '#F0EEE7',
    secondary: '#B6B5AF',
    muted: '#9AA3B3',
    subtle: '#707A8C',
    inverse: '#1F2430'
  },
  surface: {
    leftPanel: '#1F2430',
    mainPanel: '#242936',
    rightPanel: '#282E3B',
    card: '#2D3443',
    input: '#2D3443',
    popover: '#2D3443',
    muted: '#282E3B',
    selected: '#3B465B',
    selectedSubtle: '#2D3443',
    disabled: '#3A4251'
  },
  border: {
    default: '#3A4251',
    strong: '#596579'
  },
  accent: '#FFCC66',
  semantic: {
    danger: '#FF6666',
    success: '#87D96C',
    info: '#73D0FF'
  },
  button: {
    primaryBg: '#FFCC66',
    primaryBgHover: '#FFD580',
    primaryText: '#1F2430',
    dangerBg: '#C6454A',
    dangerBgHover: '#C1484D',
    dangerText: '#FFFFFF'
  },
  visual: {
    avatar: '#FFA659',
    input: '#73D0FF',
    output: '#DFBFFF',
    outputThinking: '#F29E74',
    sourceBadgeBackground: '#73D0FF',
    sourceBadgeText: '#1F2430'
  },
  gitReview: {
    syntax: ayuMirageGitReviewSyntax
  },
  terminal: {
    background: '#242936',
    foreground: '#CCCAC2',
    cursor: '#FFCC66',
    selectionBackground: '#3B465B',
    black: '#0A0000',
    red: '#F28779',
    green: '#D5FF80',
    yellow: '#FFCD66',
    blue: '#73D0FF',
    magenta: '#DFBFFF',
    cyan: '#95E6CB',
    white: '#AAB2C3',
    brightBlack: '#707A8C',
    brightRed: '#F68F82',
    brightGreen: '#D5FF80',
    brightYellow: '#FFCD66',
    brightBlue: '#73D0FF',
    brightMagenta: '#DFBFFF',
    brightCyan: '#95E6CB',
    brightWhite: '#CCCAC2'
  },
  shadow: '#0A0000'
} as const satisfies PaletteThemeRecipe

const ayuDarkRecipe = {
  colorScheme: 'dark',
  text: {
    primary: '#BFBDB6',
    strong: '#E6E3DC',
    secondary: '#A9A8A2',
    muted: '#7B8496',
    subtle: '#5A6378',
    inverse: '#0D1017'
  },
  surface: {
    leftPanel: '#0D1017',
    mainPanel: '#10141C',
    rightPanel: '#141821',
    card: '#161A24',
    input: '#161A24',
    popover: '#161A24',
    muted: '#141821',
    selected: '#24344A',
    selectedSubtle: '#161A24',
    disabled: '#242936'
  },
  border: {
    default: '#2A303B',
    strong: '#475266'
  },
  accent: '#E6B450',
  semantic: {
    danger: '#F07178',
    success: '#AAD94C',
    info: '#59C2FF'
  },
  button: {
    primaryBg: '#E6B450',
    primaryBgHover: '#F0C263',
    primaryText: '#0D1017',
    dangerBg: '#B9474D',
    dangerBgHover: '#C64D53',
    dangerText: '#FFFFFF'
  },
  visual: {
    avatar: '#FF8F40',
    input: '#59C2FF',
    output: '#D2A6FF',
    outputThinking: '#F29668',
    sourceBadgeBackground: '#59C2FF',
    sourceBadgeText: '#0D1017'
  },
  gitReview: {
    syntax: ayuDarkGitReviewSyntax
  },
  terminal: {
    background: '#10141C',
    foreground: '#BFBDB6',
    cursor: '#E6B450',
    selectionBackground: '#24344A',
    black: '#0A0000',
    red: '#F07178',
    green: '#AAD94C',
    yellow: '#FFB454',
    blue: '#59C2FF',
    magenta: '#D2A6FF',
    cyan: '#95E6CB',
    white: '#BFBDB6',
    brightBlack: '#5A6378',
    brightRed: '#F58A90',
    brightGreen: '#C0EA6C',
    brightYellow: '#FFC777',
    brightBlue: '#73D0FF',
    brightMagenta: '#DFBFFF',
    brightCyan: '#A8F0D9',
    brightWhite: '#FFFFFF'
  },
  shadow: '#0A0000'
} as const satisfies PaletteThemeRecipe

export const ayuLightTheme = createPaletteTheme(ayuLightRecipe)
export const ayuMirageTheme = createPaletteTheme(ayuMirageRecipe)
export const ayuDarkTheme = createPaletteTheme(ayuDarkRecipe)
