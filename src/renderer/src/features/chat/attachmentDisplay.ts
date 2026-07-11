import type { LucideIcon } from 'lucide-react'
import {
  Braces,
  Code2,
  CodeXml,
  File as FileIcon,
  FileCode,
  FileJson,
  FileSpreadsheet,
  FileText,
  FileType,
  ImageIcon
} from 'lucide-react'
import type { AgentInputAttachment } from '@mycopilot/protocol'
import type { ComposerAttachmentKind } from './chatAttachments'

interface AttachmentPreviewInput {
  kind: 'file' | 'image'
  mimeType?: string | null
  encoding?: AgentInputAttachment['encoding']
  data?: string
  previewData?: string | null
  previewMimeType?: string | null
}

const FILE_TYPE_LABELS: Record<string, string> = {
  astro: 'ASTRO',
  bash: 'BASH',
  bat: 'BAT',
  bib: 'BIB',
  c: 'C',
  cc: 'C++',
  cfg: 'CONFIG',
  cjs: 'CJS',
  clj: 'CLJ',
  cljc: 'CLJ',
  cljs: 'CLJS',
  cmake: 'CMAKE',
  cmd: 'CMD',
  conf: 'CONFIG',
  cpp: 'C++',
  cs: 'C#',
  css: 'CSS',
  csv: 'CSV',
  cxx: 'C++',
  dart: 'DART',
  dockerfile: 'DOCKER',
  doc: 'DOC',
  docx: 'DOCX',
  edn: 'EDN',
  editorconfig: 'CONFIG',
  elm: 'ELM',
  env: 'ENV',
  erl: 'ERL',
  ex: 'EX',
  exs: 'EXS',
  fish: 'FISH',
  fs: 'FS',
  fsi: 'FS',
  fsx: 'FSX',
  gitattributes: 'GIT',
  gitignore: 'GIT',
  go: 'GO',
  gradle: 'GRADLE',
  gql: 'GQL',
  graphql: 'GRAPHQL',
  groovy: 'GROOVY',
  h: 'C',
  hcl: 'HCL',
  hh: 'C++',
  hpp: 'C++',
  hrl: 'ERL',
  hs: 'HS',
  htm: 'HTML',
  html: 'HTML',
  hxx: 'C++',
  ini: 'INI',
  ipynb: 'IPYNB',
  java: 'JAVA',
  jl: 'JL',
  js: 'JS',
  jsx: 'JSX',
  json: 'JSON',
  jsonl: 'JSONL',
  kt: 'KT',
  kts: 'KTS',
  less: 'LESS',
  lhs: 'HS',
  lock: 'LOCK',
  log: 'LOG',
  lua: 'LUA',
  m: 'M',
  make: 'MAKE',
  markdown: 'MD',
  md: 'MD',
  mdx: 'MDX',
  mjs: 'MJS',
  mk: 'MAKE',
  ml: 'ML',
  mli: 'ML',
  nim: 'NIM',
  nims: 'NIM',
  pdf: 'PDF',
  php: 'PHP',
  pl: 'PL',
  plist: 'PLIST',
  pm: 'PM',
  ppt: 'PPT',
  pptx: 'PPTX',
  prisma: 'PRISMA',
  properties: 'CONFIG',
  proto: 'PROTO',
  ps1: 'PS1',
  py: 'PY',
  pyi: 'PYI',
  r: 'R',
  rb: 'RB',
  rc: 'RC',
  rs: 'RS',
  rst: 'RST',
  sass: 'SASS',
  scala: 'SCALA',
  scss: 'SCSS',
  sh: 'SH',
  sol: 'SOL',
  sql: 'SQL',
  sv: 'SV',
  svelte: 'SVELTE',
  svh: 'SVH',
  swift: 'SWIFT',
  tex: 'TEX',
  text: 'TXT',
  tf: 'TF',
  tfvars: 'TFVARS',
  toml: 'TOML',
  tsv: 'TSV',
  ts: 'TS',
  tsx: 'TSX',
  txt: 'TXT',
  v: 'V',
  vh: 'VH',
  vue: 'VUE',
  xml: 'XML',
  xls: 'XLS',
  xlsx: 'XLSX',
  yaml: 'YAML',
  yml: 'YAML',
  zig: 'ZIG',
  zsh: 'ZSH'
}

const ATTACHMENT_BADGE_LABELS: Record<string, string> = {
  c: 'C',
  cc: 'C++',
  cjs: 'JS',
  clj: 'CLJ',
  cljc: 'CLJ',
  cljs: 'CLJ',
  cpp: 'C++',
  cs: 'C#',
  cxx: 'C++',
  dart: 'DART',
  elm: 'ELM',
  erl: 'ERL',
  ex: 'EX',
  exs: 'EX',
  fs: 'FS',
  fsi: 'FS',
  fsx: 'FS',
  go: 'GO',
  h: 'C',
  hh: 'C++',
  hpp: 'C++',
  hrl: 'ERL',
  hs: 'HS',
  hxx: 'C++',
  ipynb: 'PY',
  java: 'JAVA',
  jl: 'JL',
  js: 'JS',
  jsx: 'JS',
  kt: 'KT',
  kts: 'KT',
  lhs: 'HS',
  lua: 'LUA',
  m: 'M',
  mjs: 'JS',
  ml: 'ML',
  mli: 'ML',
  nim: 'NIM',
  nims: 'NIM',
  php: 'PHP',
  pl: 'PL',
  pm: 'PL',
  py: 'PY',
  pyi: 'PY',
  r: 'R',
  rb: 'RB',
  rs: 'RS',
  scala: 'SC',
  sol: 'SOL',
  sv: 'SV',
  svh: 'SV',
  swift: 'SW',
  ts: 'TS',
  tsx: 'TS',
  v: 'V',
  vh: 'VH',
  zig: 'ZIG'
}

const DOCUMENT_EXTENSIONS = new Set([
  'bib',
  'doc',
  'docx',
  'log',
  'markdown',
  'md',
  'mdx',
  'pdf',
  'ppt',
  'pptx',
  'rst',
  'tex',
  'text',
  'txt'
])

const SPREADSHEET_EXTENSIONS = new Set(['csv', 'tsv', 'xls', 'xlsx'])
const JSON_EXTENSIONS = new Set(['json', 'jsonl'])
const MARKUP_EXTENSIONS = new Set(['astro', 'htm', 'html', 'svg', 'svelte', 'vue', 'xml'])
const STYLE_EXTENSIONS = new Set(['css', 'less', 'sass', 'scss'])
const SCRIPT_EXTENSIONS = new Set([
  'bash',
  'bat',
  'cmake',
  'cmd',
  'dockerfile',
  'fish',
  'gradle',
  'groovy',
  'make',
  'mk',
  'ps1',
  'sh',
  'zsh'
])
const CONFIG_EXTENSIONS = new Set([
  'cfg',
  'conf',
  'editorconfig',
  'env',
  'gitattributes',
  'gitignore',
  'hcl',
  'ini',
  'lock',
  'plist',
  'properties',
  'rc',
  'tf',
  'tfvars',
  'toml',
  'yaml',
  'yml'
])
const SCHEMA_DATA_EXTENSIONS = new Set(['edn', 'gql', 'graphql', 'prisma', 'proto', 'sql'])
const SOURCE_CODE_EXTENSIONS = new Set([
  'c',
  'cc',
  'cjs',
  'clj',
  'cljc',
  'cljs',
  'cpp',
  'cs',
  'cxx',
  'dart',
  'elm',
  'erl',
  'ex',
  'exs',
  'fs',
  'fsi',
  'fsx',
  'go',
  'h',
  'hh',
  'hpp',
  'hrl',
  'hs',
  'hxx',
  'ipynb',
  'java',
  'jl',
  'js',
  'jsx',
  'kt',
  'kts',
  'lhs',
  'lua',
  'm',
  'mjs',
  'ml',
  'mli',
  'nim',
  'nims',
  'php',
  'pl',
  'pm',
  'py',
  'pyi',
  'r',
  'rb',
  'rs',
  'scala',
  'sol',
  'sv',
  'svh',
  'swift',
  'ts',
  'tsx',
  'v',
  'vh',
  'zig'
])

export function getAttachmentExtension(name: string) {
  const normalized = name.trim().toLowerCase()
  if (!normalized.includes('.')) return ''
  return normalized.split('.').pop() ?? ''
}

export function getAttachmentTypeLabel(attachment: Pick<AgentInputAttachment, 'kind' | 'name'>) {
  if (attachment.kind === 'image') return 'IMAGE'
  const extension = getAttachmentExtension(attachment.name)
  return FILE_TYPE_LABELS[extension] ?? (extension.toUpperCase() || 'FILE')
}

export function getAttachmentBadgeLabel(extension: string) {
  return ATTACHMENT_BADGE_LABELS[extension] ?? null
}

export function getAttachmentIcon(kind: ComposerAttachmentKind, extension: string): LucideIcon {
  if (kind === 'image') return ImageIcon
  if (JSON_EXTENSIONS.has(extension)) return FileJson
  if (SPREADSHEET_EXTENSIONS.has(extension)) return FileSpreadsheet
  if (['doc', 'docx', 'pdf', 'ppt', 'pptx'].includes(extension)) return FileType
  if (DOCUMENT_EXTENSIONS.has(extension)) return FileText
  if (MARKUP_EXTENSIONS.has(extension)) return CodeXml
  if (STYLE_EXTENSIONS.has(extension) || SCRIPT_EXTENSIONS.has(extension)) return FileCode
  if (CONFIG_EXTENSIONS.has(extension) || SCHEMA_DATA_EXTENSIONS.has(extension)) return Braces
  if (SOURCE_CODE_EXTENSIONS.has(extension)) return Code2
  return FileIcon
}

export function getAttachmentPreviewUrl(attachment: AttachmentPreviewInput) {
  if (attachment.kind !== 'image') return undefined
  if (attachment.previewData && attachment.previewMimeType?.startsWith('image/')) {
    return `data:${attachment.previewMimeType};base64,${attachment.previewData}`
  }
  if (attachment.encoding !== 'base64') return undefined
  if (!attachment.mimeType?.startsWith('image/')) return undefined
  return `data:${attachment.mimeType};base64,${attachment.data ?? ''}`
}
