import type { ReactNode } from 'react'
import angularIcon from 'material-icon-theme/icons/angular.svg?url'
import astroIcon from 'material-icon-theme/icons/astro.svg?url'
import cIcon from 'material-icon-theme/icons/c.svg?url'
import consoleIcon from 'material-icon-theme/icons/console.svg?url'
import cppIcon from 'material-icon-theme/icons/cpp.svg?url'
import csharpIcon from 'material-icon-theme/icons/csharp.svg?url'
import cssIcon from 'material-icon-theme/icons/css.svg?url'
import databaseIcon from 'material-icon-theme/icons/database.svg?url'
import dockerIcon from 'material-icon-theme/icons/docker.svg?url'
import documentIcon from 'material-icon-theme/icons/document.svg?url'
import eslintIcon from 'material-icon-theme/icons/eslint.svg?url'
import fileIcon from 'material-icon-theme/icons/file.svg?url'
import gitIcon from 'material-icon-theme/icons/git.svg?url'
import goIcon from 'material-icon-theme/icons/go.svg?url'
import graphqlIcon from 'material-icon-theme/icons/graphql.svg?url'
import htmlIcon from 'material-icon-theme/icons/html.svg?url'
import imageIcon from 'material-icon-theme/icons/image.svg?url'
import javaIcon from 'material-icon-theme/icons/java.svg?url'
import javascriptIcon from 'material-icon-theme/icons/javascript.svg?url'
import jsonIcon from 'material-icon-theme/icons/json.svg?url'
import kotlinIcon from 'material-icon-theme/icons/kotlin.svg?url'
import lessIcon from 'material-icon-theme/icons/less.svg?url'
import licenseIcon from 'material-icon-theme/icons/license.svg?url'
import lockIcon from 'material-icon-theme/icons/lock.svg?url'
import logIcon from 'material-icon-theme/icons/log.svg?url'
import luaIcon from 'material-icon-theme/icons/lua.svg?url'
import makefileIcon from 'material-icon-theme/icons/makefile.svg?url'
import markdownIcon from 'material-icon-theme/icons/markdown.svg?url'
import npmIcon from 'material-icon-theme/icons/npm.svg?url'
import pdfIcon from 'material-icon-theme/icons/pdf.svg?url'
import phpIcon from 'material-icon-theme/icons/php.svg?url'
import pnpmIcon from 'material-icon-theme/icons/pnpm.svg?url'
import prettierIcon from 'material-icon-theme/icons/prettier.svg?url'
import prismaIcon from 'material-icon-theme/icons/prisma.svg?url'
import protoIcon from 'material-icon-theme/icons/proto.svg?url'
import pythonIcon from 'material-icon-theme/icons/python.svg?url'
import reactIcon from 'material-icon-theme/icons/react.svg?url'
import rubyIcon from 'material-icon-theme/icons/ruby.svg?url'
import rustIcon from 'material-icon-theme/icons/rust.svg?url'
import sassIcon from 'material-icon-theme/icons/sass.svg?url'
import settingsIcon from 'material-icon-theme/icons/settings.svg?url'
import svelteIcon from 'material-icon-theme/icons/svelte.svg?url'
import swiftIcon from 'material-icon-theme/icons/swift.svg?url'
import terraformIcon from 'material-icon-theme/icons/terraform.svg?url'
import tomlIcon from 'material-icon-theme/icons/toml.svg?url'
import tsconfigIcon from 'material-icon-theme/icons/tsconfig.svg?url'
import typescriptIcon from 'material-icon-theme/icons/typescript.svg?url'
import viteIcon from 'material-icon-theme/icons/vite.svg?url'
import vueIcon from 'material-icon-theme/icons/vue.svg?url'
import xmlIcon from 'material-icon-theme/icons/xml.svg?url'
import yamlIcon from 'material-icon-theme/icons/yaml.svg?url'
import yarnIcon from 'material-icon-theme/icons/yarn.svg?url'
import zigIcon from 'material-icon-theme/icons/zig.svg?url'

interface GitReviewFileIconProps {
  path: string
}

export function GitReviewFileIcon({ path }: GitReviewFileIconProps): ReactNode {
  return (
    <span className="git-review__file-icon" aria-hidden="true">
      <img alt="" draggable={false} src={fileIconSource(path)} />
    </span>
  )
}

const extensionIcons: Record<string, string> = {
  astro: astroIcon,
  bash: consoleIcon,
  bmp: imageIcon,
  c: cIcon,
  cc: cppIcon,
  cjs: javascriptIcon,
  cpp: cppIcon,
  cs: csharpIcon,
  css: cssIcon,
  cts: typescriptIcon,
  db: databaseIcon,
  fish: consoleIcon,
  gif: imageIcon,
  go: goIcon,
  gql: graphqlIcon,
  graphql: graphqlIcon,
  h: cIcon,
  hpp: cppIcon,
  htm: htmlIcon,
  html: htmlIcon,
  ico: imageIcon,
  java: javaIcon,
  jpeg: imageIcon,
  jpg: imageIcon,
  js: javascriptIcon,
  json: jsonIcon,
  json5: jsonIcon,
  jsonc: jsonIcon,
  jsx: reactIcon,
  kt: kotlinIcon,
  kts: kotlinIcon,
  less: lessIcon,
  lock: lockIcon,
  log: logIcon,
  lua: luaIcon,
  md: markdownIcon,
  mdx: markdownIcon,
  mjs: javascriptIcon,
  mts: typescriptIcon,
  pdf: pdfIcon,
  php: phpIcon,
  png: imageIcon,
  prisma: prismaIcon,
  proto: protoIcon,
  py: pythonIcon,
  pyi: pythonIcon,
  rb: rubyIcon,
  rs: rustIcon,
  sass: sassIcon,
  scss: sassIcon,
  sh: consoleIcon,
  sqlite: databaseIcon,
  sqlite3: databaseIcon,
  svg: imageIcon,
  svelte: svelteIcon,
  swift: swiftIcon,
  tf: terraformIcon,
  tfvars: terraformIcon,
  toml: tomlIcon,
  ts: typescriptIcon,
  tsx: reactIcon,
  txt: documentIcon,
  vue: vueIcon,
  webp: imageIcon,
  xml: xmlIcon,
  yaml: yamlIcon,
  yml: yamlIcon,
  zig: zigIcon,
  zsh: consoleIcon
}

function fileIconSource(path: string): string {
  const name = path.split('/').pop()?.toLowerCase() ?? path.toLowerCase()

  if (name === 'cargo.toml' || name === 'cargo.lock') return rustIcon
  if (name === 'package.json' || name === 'package-lock.json') return npmIcon
  if (name === 'pnpm-lock.yaml' || name === 'pnpm-workspace.yaml') return pnpmIcon
  if (name === 'yarn.lock') return yarnIcon
  if (name === 'makefile' || name === 'gnumakefile') return makefileIcon
  if (name === 'dockerfile' || name.startsWith('dockerfile.')) return dockerIcon
  if (name === '.gitignore' || name === '.gitattributes' || name === '.gitmodules') return gitIcon
  if (/^tsconfig(?:\..+)?\.json$/.test(name)) return tsconfigIcon
  if (/^vite\.config\./.test(name)) return viteIcon
  if (/^eslint\.config\./.test(name) || name.startsWith('.eslint')) return eslintIcon
  if (name.startsWith('.prettier') || /^prettier\.config\./.test(name)) return prettierIcon
  if (name === 'angular.json') return angularIcon
  if (name.startsWith('.env')) return settingsIcon
  if (/^(readme|changelog|contributing)(\.|$)/.test(name)) return markdownIcon
  if (/^(license|licence|copying)(\.|$)/.test(name)) return licenseIcon

  const separatorIndex = name.lastIndexOf('.')
  const extension = separatorIndex >= 0 ? name.slice(separatorIndex + 1) : ''
  return extensionIcons[extension] ?? fileIcon
}
