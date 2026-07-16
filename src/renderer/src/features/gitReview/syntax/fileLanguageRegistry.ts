/**
 * Canonical Shiki language identifiers emitted by the Git review language registry.
 *
 * Keep this list narrower than Shiki's complete bundle: every identifier added here becomes part
 * of the syntax highlighter's supported-language contract. `text` is the fail-closed fallback.
 */
export const GIT_REVIEW_SYNTAX_LANGUAGE_IDS = [
  'angular-html',
  'angular-ts',
  'astro',
  'bat',
  'bibtex',
  'blade',
  'c',
  'clojure',
  'cmake',
  'codeowners',
  'cpp',
  'csharp',
  'css',
  'csv',
  'dart',
  'docker',
  'dotenv',
  'elixir',
  'elm',
  'erb',
  'erlang',
  'fish',
  'fsharp',
  'git-commit',
  'git-rebase',
  'go',
  'graphql',
  'groovy',
  'handlebars',
  'haskell',
  'hcl',
  'html',
  'ini',
  'java',
  'javascript',
  'jinja',
  'json',
  'json5',
  'jsonc',
  'jsonl',
  'jsx',
  'julia',
  'just',
  'kotlin',
  'latex',
  'less',
  'log',
  'lua',
  'make',
  'markdown',
  'mdx',
  'mermaid',
  'nginx',
  'nim',
  'nix',
  'objective-c',
  'objective-cpp',
  'ocaml',
  'perl',
  'php',
  'powershell',
  'prisma',
  'proto',
  'python',
  'r',
  'rst',
  'ruby',
  'rust',
  'sass',
  'scala',
  'scss',
  'shellscript',
  'solidity',
  'sql',
  'svelte',
  'swift',
  'system-verilog',
  'terraform',
  'text',
  'toml',
  'tsx',
  'tsv',
  'typescript',
  'v',
  'verilog',
  'vue',
  'vue-html',
  'wasm',
  'wgsl',
  'xml',
  'yaml',
  'zig'
] as const

export type GitReviewSyntaxLanguageId = (typeof GIT_REVIEW_SYNTAX_LANGUAGE_IDS)[number]

export interface GitReviewFileLanguagePaths {
  readonly newPath: string
  readonly oldPath: string
}

/** Side-specific paths and languages; renames are intentionally allowed to change grammar. */
export interface GitReviewFileLanguageDescriptor extends GitReviewFileLanguagePaths {
  readonly newLanguageId: GitReviewSyntaxLanguageId
  readonly oldLanguageId: GitReviewSyntaxLanguageId
}

type LanguageMap = Readonly<Record<string, GitReviewSyntaxLanguageId>>

interface LanguageSuffixRule {
  readonly languageId: GitReviewSyntaxLanguageId
  readonly suffix: `.${string}`
}

const EXACT_FILENAME_LANGUAGES: LanguageMap = {
  '.babelrc': 'jsonc',
  '.bash_profile': 'shellscript',
  '.bashrc': 'shellscript',
  '.editorconfig': 'ini',
  '.env': 'dotenv',
  '.eslintrc': 'jsonc',
  '.gitattributes': 'text',
  '.gitignore': 'text',
  '.gitmodules': 'ini',
  '.npmignore': 'text',
  '.npmrc': 'ini',
  '.prettierignore': 'text',
  '.prettierrc': 'jsonc',
  '.profile': 'shellscript',
  '.stylelintrc': 'jsonc',
  '.swcrc': 'jsonc',
  '.yarnrc': 'ini',
  '.zprofile': 'shellscript',
  '.zshrc': 'shellscript',
  brewfile: 'ruby',
  'build.gradle': 'groovy',
  'cargo.lock': 'toml',
  'cargo.toml': 'toml',
  changelog: 'markdown',
  'cmakelists.txt': 'cmake',
  code_of_conduct: 'markdown',
  codeowners: 'codeowners',
  commit_editmsg: 'git-commit',
  'composer.json': 'json',
  'composer.lock': 'json',
  contributing: 'markdown',
  containerfile: 'docker',
  'deno.json': 'jsonc',
  'deno.jsonc': 'jsonc',
  dockerfile: 'docker',
  fastfile: 'ruby',
  'flake.lock': 'json',
  gemfile: 'ruby',
  'git-rebase-todo': 'git-rebase',
  gnumakefile: 'make',
  'gradle.properties': 'ini',
  gradlew: 'shellscript',
  'gradlew.bat': 'bat',
  jenkinsfile: 'groovy',
  'jsconfig.json': 'jsonc',
  justfile: 'just',
  makefile: 'make',
  'mix.lock': 'elixir',
  'nginx.conf': 'nginx',
  'package-lock.json': 'json',
  'package.json': 'json',
  'package.resolved': 'json',
  'pdm.lock': 'toml',
  pipfile: 'toml',
  'pipfile.lock': 'json',
  'pnpm-lock.yaml': 'yaml',
  'pnpm-workspace.yaml': 'yaml',
  podfile: 'ruby',
  'poetry.lock': 'toml',
  'pubspec.lock': 'yaml',
  'pubspec.yaml': 'yaml',
  'pyproject.toml': 'toml',
  rakefile: 'ruby',
  readme: 'markdown',
  'settings.gradle': 'groovy',
  'tsconfig.json': 'jsonc',
  'uv.lock': 'toml',
  vagrantfile: 'ruby',
  'yarn.lock': 'yaml'
}

/**
 * Compound suffixes whose grammar cannot be selected reliably from only the final extension.
 * Selection is longest-match, so future rules do not depend on object or array insertion order.
 */
const COMPOUND_SUFFIX_LANGUAGES = [
  { languageId: 'angular-html', suffix: '.component.html' },
  { languageId: 'angular-ts', suffix: '.component.ts' },
  { languageId: 'typescript', suffix: '.d.cts' },
  { languageId: 'typescript', suffix: '.d.mts' },
  { languageId: 'typescript', suffix: '.d.ts' },
  { languageId: 'blade', suffix: '.blade.php' },
  { languageId: 'vue-html', suffix: '.vue.html' }
] as const satisfies readonly LanguageSuffixRule[]

const SIMPLE_EXTENSION_LANGUAGES: LanguageMap = {
  astro: 'astro',
  bash: 'shellscript',
  bat: 'bat',
  bib: 'bibtex',
  c: 'c',
  cc: 'cpp',
  cfg: 'ini',
  cjs: 'javascript',
  clj: 'clojure',
  cljc: 'clojure',
  cljs: 'clojure',
  cmake: 'cmake',
  cmd: 'bat',
  conf: 'ini',
  cpp: 'cpp',
  cs: 'csharp',
  css: 'css',
  csv: 'csv',
  cts: 'typescript',
  cxx: 'cpp',
  dart: 'dart',
  dockerfile: 'docker',
  eex: 'elixir',
  env: 'dotenv',
  erb: 'erb',
  erl: 'erlang',
  ex: 'elixir',
  exs: 'elixir',
  fish: 'fish',
  fs: 'fsharp',
  fsi: 'fsharp',
  fsx: 'fsharp',
  go: 'go',
  gql: 'graphql',
  graphql: 'graphql',
  graphqls: 'graphql',
  gradle: 'groovy',
  groovy: 'groovy',
  h: 'c',
  handlebars: 'handlebars',
  hbs: 'handlebars',
  hcl: 'hcl',
  hh: 'cpp',
  hpp: 'cpp',
  hrl: 'erlang',
  hs: 'haskell',
  htm: 'html',
  html: 'html',
  hxx: 'cpp',
  ini: 'ini',
  java: 'java',
  jinja: 'jinja',
  jinja2: 'jinja',
  jl: 'julia',
  j2: 'jinja',
  js: 'javascript',
  json: 'json',
  json5: 'json5',
  jsonc: 'jsonc',
  jsonl: 'jsonl',
  jsx: 'jsx',
  kt: 'kotlin',
  kts: 'kotlin',
  less: 'less',
  log: 'log',
  lua: 'lua',
  m: 'objective-c',
  make: 'make',
  markdown: 'markdown',
  md: 'markdown',
  mdx: 'mdx',
  mjs: 'javascript',
  mm: 'objective-cpp',
  mmd: 'mermaid',
  mts: 'typescript',
  nim: 'nim',
  nims: 'nim',
  nix: 'nix',
  nginx: 'nginx',
  ml: 'ocaml',
  mli: 'ocaml',
  phtml: 'php',
  php: 'php',
  pl: 'perl',
  plist: 'xml',
  pm: 'perl',
  postcss: 'css',
  prisma: 'prisma',
  properties: 'ini',
  proto: 'proto',
  ps1: 'powershell',
  py: 'python',
  pyi: 'python',
  r: 'r',
  rb: 'ruby',
  rs: 'rust',
  rst: 'rst',
  sass: 'sass',
  scala: 'scala',
  scss: 'scss',
  sh: 'shellscript',
  sol: 'solidity',
  sql: 'sql',
  svelte: 'svelte',
  sv: 'system-verilog',
  svg: 'xml',
  svh: 'system-verilog',
  swift: 'swift',
  tf: 'terraform',
  tfvars: 'terraform',
  toml: 'toml',
  ts: 'typescript',
  tsv: 'tsv',
  tsx: 'tsx',
  txt: 'text',
  v: 'v',
  verilog: 'verilog',
  vh: 'verilog',
  vue: 'vue',
  wasm: 'wasm',
  wgsl: 'wgsl',
  xml: 'xml',
  yaml: 'yaml',
  yml: 'yaml',
  zig: 'zig',
  zsh: 'shellscript'
}

function basename(path: string): string {
  const normalized = path.replaceAll('\\', '/')
  const separatorIndex = normalized.lastIndexOf('/')
  return (separatorIndex >= 0 ? normalized.slice(separatorIndex + 1) : normalized).toLowerCase()
}

function resolveNamedFileFamily(fileName: string): GitReviewSyntaxLanguageId | undefined {
  if (fileName.startsWith('.env.')) return 'dotenv'
  if (fileName.startsWith('dockerfile.') || fileName.startsWith('containerfile.')) return 'docker'
  if (fileName.startsWith('makefile.')) return 'make'
  if (/^(?:js|ts)config(?:\..+)?\.json$/.test(fileName)) return 'jsonc'
  return undefined
}

function resolveCompoundSuffix(fileName: string): GitReviewSyntaxLanguageId | undefined {
  let bestMatch: LanguageSuffixRule | undefined
  for (const rule of COMPOUND_SUFFIX_LANGUAGES) {
    if (
      fileName.endsWith(rule.suffix) &&
      (!bestMatch || rule.suffix.length > bestMatch.suffix.length)
    ) {
      bestMatch = rule
    }
  }
  return bestMatch?.languageId
}

/** Resolves a repository-relative or absolute path to a canonical Shiki language identifier. */
export function resolveGitReviewSyntaxLanguage(path: string): GitReviewSyntaxLanguageId {
  const fileName = basename(path)
  if (!fileName) return 'text'

  const exactLanguage = EXACT_FILENAME_LANGUAGES[fileName]
  if (exactLanguage) return exactLanguage

  const familyLanguage = resolveNamedFileFamily(fileName)
  if (familyLanguage) return familyLanguage

  const compoundLanguage = resolveCompoundSuffix(fileName)
  if (compoundLanguage) return compoundLanguage

  const extensionIndex = fileName.lastIndexOf('.')
  if (extensionIndex < 0 || extensionIndex === fileName.length - 1) return 'text'
  return SIMPLE_EXTENSION_LANGUAGES[fileName.slice(extensionIndex + 1)] ?? 'text'
}

/** Builds the old/new language contract consumed by split and unified diff renderers. */
export function resolveGitReviewFileLanguageDescriptor({
  newPath,
  oldPath
}: GitReviewFileLanguagePaths): GitReviewFileLanguageDescriptor {
  return {
    newLanguageId: resolveGitReviewSyntaxLanguage(newPath),
    newPath,
    oldLanguageId: resolveGitReviewSyntaxLanguage(oldPath),
    oldPath
  }
}
