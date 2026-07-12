import { createPaletteTheme } from './paletteTheme'
import type { PaletteThemeRecipe } from './paletteTheme'

const alucardRecipe = {
  colorScheme: 'light',
  text: {
    primary: '#1F1F1F',
    strong: '#1F1F1F',
    secondary: '#3F3F3F',
    muted: '#6C664B',
    subtle: '#7B7560',
    inverse: '#FFFBEB'
  },
  surface: {
    leftPanel: '#EFEDDC',
    mainPanel: '#FFFBEB',
    rightPanel: '#FFFDF5',
    card: '#FFFDF5',
    input: '#F7F3E4',
    popover: '#FFFDF5',
    muted: '#EFEDDC',
    selected: '#DEDCCF',
    selectedSubtle: '#ECE9DF',
    disabled: '#CECCC0'
  },
  border: {
    default: '#CECCC0',
    strong: '#BCBAB3'
  },
  accent: '#644AC9',
  semantic: {
    danger: '#CB3A2A',
    success: '#14710A',
    info: '#036A96'
  },
  button: {
    primaryBg: '#644AC9',
    primaryBgHover: '#5238B4',
    primaryText: '#FFFFFF',
    dangerBg: '#B33124',
    dangerBgHover: '#93291F',
    dangerText: '#FFFFFF'
  },
  visual: {
    avatar: '#A3144D',
    input: '#036A96',
    output: '#644AC9',
    outputThinking: '#A3144D',
    sourceBadgeBackground: '#036A96',
    sourceBadgeText: '#FFFFFF'
  },
  terminal: {
    background: '#FFFBEB',
    foreground: '#1F1F1F',
    cursor: '#1F1F1F',
    selectionBackground: '#CFCFDE',
    black: '#FFFBEB',
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
