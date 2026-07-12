import { createPaletteTheme } from './paletteTheme'
import type { PaletteThemeRecipe } from './paletteTheme'

const everforestLightRecipe = {
  colorScheme: 'light',
  text: {
    primary: '#3F4C53',
    strong: '#2F3B42',
    secondary: '#536168',
    muted: '#5A6961',
    subtle: '#66756D',
    inverse: '#F7F7EF'
  },
  surface: {
    leftPanel: '#DEE5DA',
    mainPanel: '#E7ECE2',
    rightPanel: '#E1E7DC',
    card: '#F7F7EF',
    input: '#DCE4D8',
    popover: '#FAFBF7',
    muted: '#DEE5DA',
    selected: '#C8D7C8',
    selectedSubtle: '#D4E0D2',
    disabled: '#CBD5C8',
    infoSubtle: '#E9F0E9',
    successSubtle: '#F0F1D2',
    dangerSubtle: '#FDE3DA'
  },
  border: {
    default: '#B1BEB0',
    strong: '#839183'
  },
  accent: '#2F789E',
  semantic: {
    danger: '#C43D3B',
    success: '#26775A',
    info: '#2F789E'
  },
  button: {
    primaryBg: '#2F789E',
    primaryBgHover: '#286987',
    primaryText: '#FFFFFF',
    dangerBg: '#B93634',
    dangerBgHover: '#982D2B',
    dangerText: '#FFFFFF'
  },
  visual: {
    avatar: '#8DA101',
    input: '#2F789E',
    output: '#8DA101',
    outputThinking: '#A94F91',
    sourceBadgeBackground: '#2F789E',
    sourceBadgeText: '#FFFFFF'
  },
  terminal: {
    background: '#FDF6E3',
    foreground: '#5C6A72',
    cursor: '#5C6A72',
    selectionBackground: '#EAEDC8',
    black: '#5C6A72',
    red: '#F85552',
    green: '#8DA101',
    yellow: '#DFA000',
    blue: '#3A94C5',
    magenta: '#DF69BA',
    cyan: '#35A77C',
    white: '#A6B0A0',
    brightBlack: '#829181',
    brightRed: '#F85552',
    brightGreen: '#8DA101',
    brightYellow: '#DFA000',
    brightBlue: '#3A94C5',
    brightMagenta: '#DF69BA',
    brightCyan: '#35A77C',
    brightWhite: '#5C6A72'
  },
  shadow: '#5C6A72'
} as const satisfies PaletteThemeRecipe

const everforestDarkRecipe = {
  colorScheme: 'dark',
  text: {
    primary: '#D3C6AA',
    strong: '#E9E8D2',
    secondary: '#B8B29E',
    muted: '#9DA9A0',
    subtle: '#859289',
    inverse: '#232A2E'
  },
  surface: {
    leftPanel: '#232A2E',
    mainPanel: '#2D353B',
    rightPanel: '#293136',
    card: '#3D484D',
    input: '#3D484D',
    popover: '#3D484D',
    muted: '#343F44',
    selected: '#543A48',
    selectedSubtle: '#3D484D',
    disabled: '#475258',
    infoSubtle: '#3A515D',
    successSubtle: '#425047',
    dangerSubtle: '#514045'
  },
  border: {
    default: '#4F585E',
    strong: '#56635F'
  },
  accent: '#7FBBB3',
  semantic: {
    danger: '#E67E80',
    success: '#A7C080',
    info: '#7FBBB3'
  },
  button: {
    primaryBg: '#7FBBB3',
    primaryBgHover: '#83C092',
    primaryText: '#232A2E',
    dangerBg: '#E67E80',
    dangerBgHover: '#F08A8C',
    dangerText: '#232A2E'
  },
  visual: {
    avatar: '#A7C080',
    input: '#7FBBB3',
    output: '#A7C080',
    outputThinking: '#D699B6',
    sourceBadgeBackground: '#7FBBB3',
    sourceBadgeText: '#232A2E'
  },
  terminal: {
    background: '#2D353B',
    foreground: '#D3C6AA',
    cursor: '#D3C6AA',
    selectionBackground: '#543A48',
    black: '#232A2E',
    red: '#E67E80',
    green: '#A7C080',
    yellow: '#DBBC7F',
    blue: '#7FBBB3',
    magenta: '#D699B6',
    cyan: '#83C092',
    white: '#D3C6AA',
    brightBlack: '#7A8478',
    brightRed: '#E67E80',
    brightGreen: '#A7C080',
    brightYellow: '#DBBC7F',
    brightBlue: '#7FBBB3',
    brightMagenta: '#D699B6',
    brightCyan: '#83C092',
    brightWhite: '#E9E8D2'
  },
  shadow: '#1E2326'
} as const satisfies PaletteThemeRecipe

export const everforestLightTheme = createPaletteTheme(everforestLightRecipe)
export const everforestDarkTheme = createPaletteTheme(everforestDarkRecipe)
