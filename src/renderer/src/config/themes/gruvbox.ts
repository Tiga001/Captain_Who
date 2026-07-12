import { createPaletteTheme } from './paletteTheme'
import type { PaletteThemeRecipe } from './paletteTheme'

const gruvboxLightRecipe = {
  colorScheme: 'light',
  text: {
    primary: '#3C3836',
    strong: '#282828',
    secondary: '#504945',
    muted: '#665C54',
    subtle: '#7C6F64',
    inverse: '#FBF1C7'
  },
  surface: {
    leftPanel: '#E6E3DB',
    mainPanel: '#F6F4EC',
    rightPanel: '#F2F0E8',
    card: '#FAF8F2',
    input: '#F0EDE5',
    popover: '#FCFBF7',
    muted: '#ECE8DE',
    selected: '#D7D0C2',
    selectedSubtle: '#E5E0D5',
    disabled: '#C8C0B2'
  },
  border: {
    default: '#D7D0C2',
    strong: '#B5AA99'
  },
  accent: '#076678',
  semantic: {
    danger: '#9D0006',
    success: '#5F6A0B',
    info: '#076678'
  },
  button: {
    primaryBg: '#076678',
    primaryBgHover: '#055568',
    primaryText: '#FFFFFF',
    dangerBg: '#9D0006',
    dangerBgHover: '#7D0005',
    dangerText: '#FFFFFF'
  },
  visual: {
    avatar: '#AF3A03',
    input: '#076678',
    output: '#8F3F71',
    outputThinking: '#AF3A03',
    sourceBadgeBackground: '#076678',
    sourceBadgeText: '#FFFFFF'
  },
  terminal: {
    background: '#FBF1C7',
    foreground: '#3C3836',
    cursor: '#3C3836',
    selectionBackground: '#D5C4A1',
    black: '#FBF1C7',
    red: '#CC241D',
    green: '#98971A',
    yellow: '#D79921',
    blue: '#458588',
    magenta: '#B16286',
    cyan: '#689D6A',
    white: '#7C6F64',
    brightBlack: '#928374',
    brightRed: '#9D0006',
    brightGreen: '#79740E',
    brightYellow: '#B57614',
    brightBlue: '#076678',
    brightMagenta: '#8F3F71',
    brightCyan: '#427B58',
    brightWhite: '#3C3836'
  },
  shadow: '#282828'
} as const satisfies PaletteThemeRecipe

const gruvboxDarkRecipe = {
  colorScheme: 'dark',
  text: {
    primary: '#EBDBB2',
    strong: '#FBF1C7',
    secondary: '#D5C4A1',
    muted: '#BDAE93',
    subtle: '#928374',
    inverse: '#282828'
  },
  surface: {
    leftPanel: '#1D2021',
    mainPanel: '#282828',
    rightPanel: '#32302F',
    card: '#3C3836',
    input: '#3C3836',
    popover: '#3C3836',
    muted: '#3C3836',
    selected: '#504945',
    selectedSubtle: '#3C3836',
    disabled: '#504945'
  },
  border: {
    default: '#504945',
    strong: '#665C54'
  },
  accent: '#83A598',
  semantic: {
    danger: '#FE5A43',
    success: '#B8BB26',
    info: '#83A598'
  },
  button: {
    primaryBg: '#FABD2F',
    primaryBgHover: '#D79921',
    primaryText: '#282828',
    dangerBg: '#FE5A43',
    dangerBgHover: '#FF6B55',
    dangerText: '#282828'
  },
  visual: {
    avatar: '#FE8019',
    input: '#83A598',
    output: '#D3869B',
    outputThinking: '#FE8019',
    sourceBadgeBackground: '#FABD2F',
    sourceBadgeText: '#282828'
  },
  terminal: {
    background: '#282828',
    foreground: '#EBDBB2',
    cursor: '#EBDBB2',
    selectionBackground: '#504945',
    black: '#282828',
    red: '#CC241D',
    green: '#98971A',
    yellow: '#D79921',
    blue: '#458588',
    magenta: '#B16286',
    cyan: '#689D6A',
    white: '#A89984',
    brightBlack: '#928374',
    brightRed: '#FB4934',
    brightGreen: '#B8BB26',
    brightYellow: '#FABD2F',
    brightBlue: '#83A598',
    brightMagenta: '#D3869B',
    brightCyan: '#8EC07C',
    brightWhite: '#EBDBB2'
  },
  shadow: '#1D2021'
} as const satisfies PaletteThemeRecipe

export const gruvboxLightTheme = createPaletteTheme(gruvboxLightRecipe)
export const gruvboxDarkTheme = createPaletteTheme(gruvboxDarkRecipe)
