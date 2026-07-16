import type { CSSProperties } from 'react'
import type { FrontendThemeId } from '../../../config/frontendTheme'

export interface GitReviewSyntaxPalette {
  readonly attribute: string
  readonly comment: string
  readonly constant: string
  readonly foreground: string
  readonly function: string
  readonly invalid: string
  readonly keyword: string
  readonly number: string
  readonly regexp: string
  readonly string: string
  readonly tag: string
  readonly type: string
  readonly variable: string
}

interface GitReviewSyntaxPaletteSeed {
  readonly attribute?: string
  readonly comment: string
  readonly constant?: string
  readonly foreground: string
  readonly function: string
  readonly keyword: string
  readonly number: string
  readonly regexp?: string
  readonly string: string
  readonly tag?: string
  readonly type: string
  readonly variable: string
}

const INVALID_FOREGROUND = 'var(--mc-color-text-danger)'

function definePalette(seed: GitReviewSyntaxPaletteSeed): GitReviewSyntaxPalette {
  return Object.freeze({
    attribute: seed.attribute ?? seed.type,
    comment: seed.comment,
    constant: seed.constant ?? seed.number,
    foreground: seed.foreground,
    function: seed.function,
    invalid: INVALID_FOREGROUND,
    keyword: seed.keyword,
    number: seed.number,
    regexp: seed.regexp ?? seed.string,
    string: seed.string,
    tag: seed.tag ?? seed.keyword,
    type: seed.type,
    variable: seed.variable
  })
}

/**
 * Foreground-only palettes. Codex values were verified from the local app bundle; standard theme
 * values were sampled from the corresponding Shiki 4 registrations. Custom product themes derive
 * from their existing MyCopilot UI and terminal colors.
 */
const syntaxPalettes = {
  'alucard-light': definePalette({
    attribute: '#036A96',
    comment: '#6C664B',
    constant: '#846E15',
    foreground: '#1F1F1F',
    function: '#644AC9',
    keyword: '#CB3A2A',
    number: '#036A96',
    regexp: '#036A96',
    string: '#14710A',
    tag: '#CB3A2A',
    type: '#644AC9',
    variable: '#846E15'
  }),
  'ayu-dark': definePalette({
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
  }),
  'ayu-light': definePalette({
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
  }),
  'ayu-mirage': definePalette({
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
  }),
  'catppuccin-latte': definePalette({
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
  }),
  'catppuccin-mocha': definePalette({
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
  }),
  'codex-dark': definePalette({
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
  }),
  'codex-light': definePalette({
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
  }),
  'crab-light': definePalette({
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
  }),
  dracula: definePalette({
    attribute: '#50FA7B',
    comment: '#6272A4',
    foreground: '#F8F8F2',
    function: '#50FA7B',
    keyword: '#FF79C6',
    number: '#BD93F9',
    regexp: '#FF5555',
    string: '#F1FA8C',
    tag: '#FF79C6',
    type: '#8BE9FD',
    variable: '#F8F8F2'
  }),
  'everforest-dark': definePalette({
    attribute: '#A7C080',
    comment: '#859289',
    foreground: '#D3C6AA',
    function: '#A7C080',
    keyword: '#E67E80',
    number: '#D699B6',
    regexp: '#DBBC7F',
    string: '#DBBC7F',
    tag: '#E69875',
    type: '#83C092',
    variable: '#D3C6AA'
  }),
  'everforest-light': definePalette({
    attribute: '#8DA101',
    comment: '#939F91',
    foreground: '#5C6A72',
    function: '#8DA101',
    keyword: '#F85552',
    number: '#DF69BA',
    regexp: '#DFA000',
    string: '#DFA000',
    tag: '#F57D26',
    type: '#35A77C',
    variable: '#5C6A72'
  }),
  'github-dark-default': definePalette({
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
  }),
  'github-light-default': definePalette({
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
  }),
  'gruvbox-dark-medium': definePalette({
    attribute: '#FABD2F',
    comment: '#928374',
    foreground: '#EBDBB2',
    function: '#FABD2F',
    keyword: '#FB4934',
    number: '#D3869B',
    regexp: '#FE8019',
    string: '#B8BB26',
    tag: '#8EC07C',
    type: '#FABD2F',
    variable: '#83A598'
  }),
  'gruvbox-light-medium': definePalette({
    attribute: '#B57614',
    comment: '#928374',
    foreground: '#3C3836',
    function: '#B57614',
    keyword: '#9D0006',
    number: '#8F3F71',
    regexp: '#AF3A03',
    string: '#79740E',
    tag: '#427B58',
    type: '#B57614',
    variable: '#076678'
  }),
  'one-dark-pro': definePalette({
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
  }),
  'one-light': definePalette({
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
  }),
  'zju-light': definePalette({
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
  })
} as const

type GitReviewSyntaxPaletteId = keyof typeof syntaxPalettes

export const syntaxPaletteIdByFrontendThemeId = {
  'alucard-light': 'alucard-light',
  'ayu-dark': 'ayu-dark',
  'ayu-light': 'ayu-light',
  'ayu-mirage-dark': 'ayu-mirage',
  'catppuccin-latte-light': 'catppuccin-latte',
  'catppuccin-mocha-dark': 'catppuccin-mocha',
  'classic-dark': 'codex-dark',
  'classic-light': 'codex-light',
  'crab-light': 'crab-light',
  'dracula-dark': 'dracula',
  'everforest-dark': 'everforest-dark',
  'everforest-light': 'everforest-light',
  'github-dark': 'github-dark-default',
  'github-light': 'github-light-default',
  'gruvbox-dark': 'gruvbox-dark-medium',
  'gruvbox-light': 'gruvbox-light-medium',
  'one-dark': 'one-dark-pro',
  'one-light': 'one-light',
  'zju-light': 'zju-light'
} as const satisfies Record<FrontendThemeId, GitReviewSyntaxPaletteId>

const syntaxThemeStyles = Object.fromEntries(
  Object.entries(syntaxPalettes).map(([id, palette]) => [id, createSyntaxThemeStyle(palette)])
) as Record<GitReviewSyntaxPaletteId, CSSProperties>

export function getGitReviewSyntaxThemeStyle(themeId: FrontendThemeId): CSSProperties {
  return syntaxThemeStyles[syntaxPaletteIdByFrontendThemeId[themeId]]
}

function createSyntaxThemeStyle(palette: GitReviewSyntaxPalette): CSSProperties {
  return Object.freeze({
    '--git-review-syntax-attribute': palette.attribute,
    '--git-review-syntax-background': 'transparent',
    '--git-review-syntax-comment': palette.comment,
    '--git-review-syntax-constant': palette.constant,
    '--git-review-syntax-foreground': palette.foreground,
    '--git-review-syntax-function': palette.function,
    '--git-review-syntax-invalid': palette.invalid,
    '--git-review-syntax-keyword': palette.keyword,
    '--git-review-syntax-number': palette.number,
    '--git-review-syntax-regexp': palette.regexp,
    '--git-review-syntax-string': palette.string,
    '--git-review-syntax-tag': palette.tag,
    '--git-review-syntax-type': palette.type,
    '--git-review-syntax-variable': palette.variable
  }) as CSSProperties
}
