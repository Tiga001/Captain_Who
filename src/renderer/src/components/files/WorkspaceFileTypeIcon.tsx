import { createFileTreeIconResolver, getBuiltInSpriteSheet } from '@pierre/trees'
import './WorkspaceFileTypeIcon.css'

const resolver = createFileTreeIconResolver({ colored: true, set: 'complete' })
// The bundled sprite is trusted application code. Keep only the requested symbol's paths in
// each SVG, rather than duplicating the complete sprite or creating document-global IDs.
const symbols = new Map(
  Array.from(
    getBuiltInSpriteSheet('complete').matchAll(/<symbol id="([^"]+)"[^>]*>([\s\S]*?)<\/symbol>/g),
    ([, id, contents]) => [id, contents]
  )
)

// @pierre/trees exposes its resolver and sprite publicly, but keeps its color styles inside
// the tree's shadow root. Match that palette here so standalone icons look like the file tree.
const colorTokens: Readonly<Record<string, string>> = {
  astro: 'purple',
  babel: 'yellow',
  bash: 'green',
  biome: 'blue',
  bootstrap: 'indigo',
  browserslist: 'yellow',
  bun: 'mauve',
  c: 'blue',
  cpp: 'blue',
  claude: 'orange',
  css: 'indigo',
  database: 'purple',
  docker: 'blue',
  eslint: 'indigo',
  git: 'vermilion',
  go: 'cyan',
  graphql: 'pink',
  html: 'orange',
  image: 'pink',
  javascript: 'yellow',
  json: 'orange',
  markdown: 'green',
  mcp: 'teal',
  npm: 'red',
  oxc: 'cyan',
  postcss: 'red',
  prettier: 'teal',
  python: 'blue',
  react: 'cyan',
  ruby: 'red',
  rust: 'orange',
  sass: 'pink',
  svg: 'orange',
  svelte: 'red',
  svgo: 'green',
  swift: 'orange',
  table: 'teal',
  tailwind: 'cyan',
  terraform: 'indigo',
  typescript: 'blue',
  vite: 'purple',
  vscode: 'blue',
  vue: 'green',
  wasm: 'indigo',
  webpack: 'blue',
  yml: 'red',
  zig: 'orange'
}

export function WorkspaceFileTypeIcon({
  path,
  className = ''
}: {
  path: string
  className?: string
}) {
  const icon = resolver.resolveIcon('file-tree-icon-file', path.replaceAll('\\', '/'))
  return (
    <svg
      aria-hidden="true"
      className={`workspace-file-type-icon ${className}`}
      data-file-icon={icon.token ?? 'default'}
      data-icon-color={colorTokens[icon.token ?? ''] ?? 'gray'}
      focusable="false"
      viewBox={icon.viewBox ?? '0 0 16 16'}
      dangerouslySetInnerHTML={{ __html: symbols.get(icon.name) ?? '' }}
    />
  )
}
