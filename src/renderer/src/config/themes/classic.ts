import type { FrontendTheme } from './types'
import { createGitReviewColors } from './gitReviewTheme'

const classicLightGitReviewSyntax = {
  attribute: '#751ED9',
  comment: '#666666',
  constant: '#BD5800',
  foreground: '#0D0D0D',
  function: '#751ED9',
  keyword: '#D53538',
  number: '#0071EA',
  regexp: '#001BCB',
  string: '#008809',
  tag: '#D53538',
  type: '#751ED9',
  variable: '#BD5800'
} as const

const classicDarkGitReviewSyntax = {
  attribute: '#B06DFF',
  comment: '#999999',
  constant: '#FA994C',
  foreground: '#FCFCFC',
  function: '#B06DFF',
  keyword: '#F67576',
  number: '#6DCBF4',
  regexp: '#3D8DFF',
  string: '#85DF7B',
  tag: '#F67576',
  type: '#B06DFF',
  variable: '#FA994C'
} as const

const classicStatusColors = {
  successText: '#37c86a',
  successSurface: 'color-mix(in srgb, #37c86a 18%, transparent)'
} as const

const classicDataVizColors = {
  input: '#3269e8',
  output: '#2457df',
  outputThinking: '#8f3f71'
} as const

const classicSourceBadgeColors = {
  text: '#ffffff',
  background: '#2f6fab'
} as const

const classicMediaViewerColors = {
  foreground: 'rgba(255, 255, 255, 0.94)',
  backdrop: 'rgba(0, 0, 0, 0.86)',
  controlBackground: 'rgba(44, 44, 44, 0.92)',
  controlHover: 'rgba(70, 70, 70, 0.96)',
  focusRing: 'rgba(255, 255, 255, 0.78)',
  menuBackground: 'rgba(32, 32, 32, 0.98)',
  menuBorder: 'rgba(255, 255, 255, 0.12)'
} as const

const classicEffectColors = {
  shimmerHighlight: 'rgba(255, 255, 255, 0.72)'
} as const

export const classicLightTheme = {
  colors: {
    text: {
      primary: '#3F3F46',
      strong: '#1A1C1F',
      secondary: '#4f5660',
      muted: '#8b95a1',
      subtle: '#979ca4',
      inverse: '#ffffff',
      danger: '#b42318',
      accent: '#0169CC'
    },
    icon: {
      default: '#1A1C1F',
      muted: '#697381',
      subtle: '#8f949c',
      accent: '#0169CC',
      danger: '#ff2a16',
      success: '#008768'
    },
    avatar: {
      background: '#D85A00'
    },
    surface: {
      leftPanel: '#F7F7F5',
      mainPanel: '#FFFFFF',
      rightPanel: '#FFFFFF',
      card: '#ffffff',
      input: '#ffffff',
      popover: '#ffffff',
      muted: '#f3f4f5',
      selected: '#e9eaec',
      selectedSubtle: '#f1f2f4',
      infoSubtle: '#eef8ff',
      successSubtle: '#dcfaee',
      neutralSubtle: '#f3f4f6',
      disabled: '#e5e7eb',
      dangerSubtle: '#ffe9ea',
      dangerSoft: 'rgba(255, 42, 22, 0.08)',
      errorSubtle: 'rgba(255, 59, 48, 0.08)',
      glass: 'rgba(255, 255, 255, 0.94)',
      glassStrong: 'rgba(255, 255, 255, 0.96)',
      glassInput: 'rgba(255, 255, 255, 0.76)'
    },
    border: {
      hairline: 'rgba(24, 24, 27, 0.08)',
      subtle: 'rgba(31, 35, 41, 0.08)',
      default: '#d4dde8',
      strong: '#cdd6e1',
      error: 'rgba(255, 59, 48, 0.18)',
      rightToolbarDivider: 'rgba(255, 255, 255, 0.05)'
    },
    button: {
      primaryBg: '#142231',
      primaryBgHover: '#1d3044',
      primaryText: '#ffffff',
      secondaryBg: '#ffffff',
      secondaryBgHover: '#f8fafc',
      secondaryText: '#1f2d3d',
      dangerBg: '#d92d20',
      dangerBgHover: '#b42318',
      dangerText: '#ffffff',
      dangerSoftText: '#ff272e',
      dangerSoftBg: '#ffe9ea',
      dangerSoftBgHover: '#fbd9dc'
    },
    control: {
      selectedBackground: '#0169CC',
      selectedText: '#ffffff'
    },
    sidebar: {
      textSecondary: '#2F3338',
      textActive: '#1A1C1F',
      translucentTint: '#F7F7F5'
    },
    settings: {
      contentTitle: '#1A1C1F',
      contentText: '#2F3338',
      contentMuted: '#8b95a1'
    },
    state: {
      hover: 'rgba(31, 35, 41, 0.055)',
      active: 'rgba(31, 35, 41, 0.075)',
      focusRing: 'rgba(51, 156, 255, 0.5)',
      resizeHandle: 'rgba(51, 156, 255, 0.55)',
      overlay: 'rgba(17, 24, 39, 0.18)',
      overlayLight: 'rgba(248, 248, 248, 0.45)'
    },
    status: classicStatusColors,
    diff: {
      additionText: '#16a34a',
      deletionText: '#b42318'
    },
    gitReview: createGitReviewColors({
      colorScheme: 'light',
      surface: {
        panel: '#FFFFFF',
        muted: '#f3f4f5'
      },
      text: {
        primary: '#3F3F46',
        secondary: '#4f5660',
        muted: '#8b95a1'
      },
      border: {
        hairline: 'rgba(24, 24, 27, 0.08)',
        subtle: 'rgba(31, 35, 41, 0.08)',
        default: '#d4dde8'
      },
      stateHover: 'rgba(31, 35, 41, 0.055)',
      additionText: '#16a34a',
      deletionText: '#b42318',
      syntax: classicLightGitReviewSyntax
    }),
    dataViz: classicDataVizColors,
    sourceBadge: classicSourceBadgeColors,
    effect: classicEffectColors,
    terminal: {
      background: '#FFFFFF',
      foreground: '#3F3F46',
      cursor: '#3F3F46',
      selectionBackground: '#e9eaec',
      black: '#f3f4f5',
      red: '#b42318',
      green: '#008768',
      yellow: '#1A1C1F',
      blue: '#0169CC',
      magenta: '#0169CC',
      cyan: '#0169CC',
      white: '#3F3F46',
      brightBlack: '#8b95a1',
      brightRed: '#b42318',
      brightGreen: '#008768',
      brightYellow: '#1A1C1F',
      brightBlue: '#0169CC',
      brightMagenta: '#0169CC',
      brightCyan: '#0169CC',
      brightWhite: '#3F3F46'
    },
    mediaViewer: classicMediaViewerColors
  },
  shadow: {
    none: 'none',
    hairline: '0 1px 2px rgba(25, 28, 33, 0.03)',
    focus: '0 0 0 3px rgba(45, 105, 210, 0.12)',
    composer: '0 18px 46px rgba(20, 23, 28, 0.1), 0 2px 8px rgba(20, 23, 28, 0.04)',
    popover: '0 18px 48px rgba(20, 23, 28, 0.14), 0 3px 10px rgba(20, 23, 28, 0.06)',
    popoverLarge: '0 22px 58px rgba(20, 23, 28, 0.16), 0 6px 18px rgba(20, 23, 28, 0.08)',
    sidebarMenu: '0 22px 48px rgba(15, 23, 42, 0.14), 0 3px 10px rgba(15, 23, 42, 0.08)',
    card: '0 10px 28px rgba(20, 23, 28, 0.08)',
    dialog: '0 32px 72px rgba(15, 23, 42, 0.22), 0 8px 24px rgba(15, 23, 42, 0.12)',
    mediaViewerMenu: '0 16px 42px rgba(0, 0, 0, 0.4)'
  }
} as const satisfies FrontendTheme

export const classicDarkTheme = {
  colors: {
    text: {
      primary: '#FCFCFC',
      strong: '#FCFCFC',
      secondary: '#E0E0E0',
      muted: '#969BA3',
      subtle: '#767B84',
      inverse: '#111111',
      danger: '#FF6B5F',
      accent: '#0169CC'
    },
    icon: {
      default: '#FCFCFC',
      muted: '#A7ADB6',
      subtle: '#7F858D',
      accent: '#0169CC',
      danger: '#FF5F54',
      success: '#35D493'
    },
    avatar: {
      background: '#D85A00'
    },
    surface: {
      leftPanel: '#171717',
      mainPanel: '#101010',
      rightPanel: '#141414',
      card: '#111111',
      input: '#111111',
      popover: '#111111',
      muted: '#1F1F20',
      selected: '#2B2C2E',
      selectedSubtle: '#1D2227',
      infoSubtle: 'rgba(1, 105, 204, 0.18)',
      successSubtle: 'rgba(53, 212, 147, 0.16)',
      neutralSubtle: '#202123',
      disabled: '#303236',
      dangerSubtle: 'rgba(255, 95, 84, 0.14)',
      dangerSoft: 'rgba(255, 95, 84, 0.14)',
      errorSubtle: 'rgba(255, 95, 84, 0.13)',
      glass: 'rgba(22, 22, 22, 0.92)',
      glassStrong: 'rgba(24, 24, 24, 0.96)',
      glassInput: 'rgba(255, 255, 255, 0.08)'
    },
    border: {
      hairline: 'rgba(255, 255, 255, 0.09)',
      subtle: 'rgba(255, 255, 255, 0.09)',
      default: '#34363A',
      strong: '#464A51',
      error: 'rgba(255, 95, 84, 0.25)',
      rightToolbarDivider: 'rgba(255, 255, 255, 0.05)'
    },
    button: {
      primaryBg: '#0169CC',
      primaryBgHover: '#0878E4',
      primaryText: '#FCFCFC',
      secondaryBg: '#1A1B1D',
      secondaryBgHover: '#232528',
      secondaryText: '#FCFCFC',
      dangerBg: '#D94335',
      dangerBgHover: '#F05245',
      dangerText: '#FFFFFF',
      dangerSoftText: '#FF8379',
      dangerSoftBg: 'rgba(255, 95, 84, 0.14)',
      dangerSoftBgHover: 'rgba(255, 95, 84, 0.2)'
    },
    control: {
      selectedBackground: '#0169CC',
      selectedText: '#FCFCFC'
    },
    sidebar: {
      textSecondary: '#ECECEC',
      textActive: '#FCFCFC',
      translucentTint: '#000000'
    },
    settings: {
      contentTitle: '#FCFCFC',
      contentText: '#ECECEC',
      contentMuted: '#969BA3'
    },
    state: {
      hover: 'rgba(255, 255, 255, 0.075)',
      active: 'rgba(255, 255, 255, 0.11)',
      focusRing: '#0169CC',
      resizeHandle: '#0169CC',
      overlay: 'rgba(0, 0, 0, 0.5)',
      overlayLight: 'rgba(0, 0, 0, 0.44)'
    },
    status: classicStatusColors,
    diff: {
      additionText: '#16a34a',
      deletionText: '#FF6B5F'
    },
    gitReview: createGitReviewColors({
      colorScheme: 'dark',
      surface: {
        panel: '#141414',
        muted: '#1F1F20'
      },
      text: {
        primary: '#FCFCFC',
        secondary: '#E0E0E0',
        muted: '#969BA3'
      },
      border: {
        hairline: 'rgba(255, 255, 255, 0.09)',
        subtle: 'rgba(255, 255, 255, 0.09)',
        default: '#34363A'
      },
      stateHover: 'rgba(255, 255, 255, 0.075)',
      additionText: '#16a34a',
      deletionText: '#FF6B5F',
      syntax: classicDarkGitReviewSyntax
    }),
    dataViz: classicDataVizColors,
    sourceBadge: classicSourceBadgeColors,
    effect: classicEffectColors,
    terminal: {
      background: '#141414',
      foreground: '#FCFCFC',
      cursor: '#FCFCFC',
      selectionBackground: '#2B2C2E',
      black: '#1F1F20',
      red: '#FF6B5F',
      green: '#35D493',
      yellow: '#FCFCFC',
      blue: '#0169CC',
      magenta: '#0169CC',
      cyan: '#0169CC',
      white: '#FCFCFC',
      brightBlack: '#969BA3',
      brightRed: '#FF6B5F',
      brightGreen: '#35D493',
      brightYellow: '#FCFCFC',
      brightBlue: '#0169CC',
      brightMagenta: '#0169CC',
      brightCyan: '#0169CC',
      brightWhite: '#FCFCFC'
    },
    mediaViewer: classicMediaViewerColors
  },
  shadow: {
    none: 'none',
    hairline: '0 1px 2px rgba(0, 0, 0, 0.28)',
    focus: '0 0 0 3px rgba(1, 105, 204, 0.22)',
    composer: '0 18px 46px rgba(0, 0, 0, 0.36), 0 2px 8px rgba(0, 0, 0, 0.28)',
    popover: '0 18px 48px rgba(0, 0, 0, 0.48), 0 3px 10px rgba(0, 0, 0, 0.32)',
    popoverLarge: '0 22px 58px rgba(0, 0, 0, 0.54), 0 6px 18px rgba(0, 0, 0, 0.36)',
    sidebarMenu: '0 22px 48px rgba(0, 0, 0, 0.5), 0 3px 10px rgba(0, 0, 0, 0.34)',
    card: '0 10px 28px rgba(0, 0, 0, 0.36)',
    dialog: '0 32px 72px rgba(0, 0, 0, 0.62), 0 8px 24px rgba(0, 0, 0, 0.44)',
    mediaViewerMenu: '0 16px 42px rgba(0, 0, 0, 0.4)'
  }
} as const satisfies FrontendTheme
