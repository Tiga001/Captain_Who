/* eslint-disable @typescript-eslint/explicit-function-return-type -- Documentation checker is runtime-validated JavaScript. */
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const repositoryRoot = path.resolve(fileURLToPath(new URL('..', import.meta.url)))
const docsRoot = path.join(repositoryRoot, 'docs')
const packageJson = JSON.parse(readFileSync(path.join(repositoryRoot, 'package.json'), 'utf8'))
const failures = []

function readJson(repositoryPath) {
  return JSON.parse(readFileSync(path.join(repositoryRoot, repositoryPath), 'utf8'))
}

function walkMarkdown(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const absolutePath = path.join(directory, entry.name)
    return entry.isDirectory()
      ? walkMarkdown(absolutePath)
      : entry.isFile() && entry.name.endsWith('.md')
        ? [absolutePath]
        : []
  })
}

function relative(file) {
  return path.relative(repositoryRoot, file).split(path.sep).join('/')
}

function fail(file, message) {
  failures.push(`${relative(file)}: ${message}`)
}

function stripFencedCode(markdown) {
  return markdown.replace(/^```[^\n]*\n[\s\S]*?^```\s*$/gm, '')
}

function parseFrontMatter(file, markdown) {
  const match = /^---\n([\s\S]*?)\n---\n/.exec(markdown)
  if (!match) {
    fail(file, 'missing YAML front matter')
    return null
  }

  const metadata = Object.fromEntries(
    match[1]
      .split('\n')
      .map((line) => line.match(/^([a-z_]+):\s*(.+)$/))
      .filter(Boolean)
      .map((lineMatch) => [lineMatch[1], lineMatch[2].trim()])
  )
  for (const key of ['status', 'audience', 'owner', 'last_verified']) {
    if (!metadata[key]) fail(file, `missing front matter field ${key}`)
  }
  if (
    metadata.status &&
    !['current', 'draft', 'historical', 'deprecated'].includes(metadata.status)
  ) {
    fail(file, `unsupported status ${metadata.status}`)
  }
  if (metadata.last_verified && !/^\d{4}-\d{2}-\d{2}$/.test(metadata.last_verified)) {
    fail(file, `last_verified must be YYYY-MM-DD, found ${metadata.last_verified}`)
  }
  return metadata
}

function resolveMarkdownTarget(file, rawTarget) {
  let target = rawTarget.trim().replace(/^<|>$/g, '')
  if (!target || target.startsWith('#') || /^[a-z][a-z0-9+.-]*:/i.test(target)) return null
  target = target.split('#')[0]
  try {
    target = decodeURIComponent(target)
  } catch {
    fail(file, `link target is not valid percent-encoding: ${rawTarget}`)
    return null
  }
  return path.resolve(path.dirname(file), target)
}

const documentationFiles = walkMarkdown(docsRoot).sort()
const markdownFiles = [path.join(repositoryRoot, 'README.md'), ...documentationFiles]
const indexedTargets = new Set()
const currentDocuments = []

const rootReadme = path.join(repositoryRoot, 'README.md')
const docsReadme = path.join(docsRoot, 'README.md')
if (!readFileSync(rootReadme, 'utf8').includes('# Captain Who')) {
  fail(rootReadme, 'missing canonical product heading “Captain Who”')
}
if (!readFileSync(docsReadme, 'utf8').includes('# Captain Who 开发文档')) {
  fail(docsReadme, 'missing canonical development documentation heading “Captain Who 开发文档”')
}

for (const file of documentationFiles) {
  const basename = path.basename(file)
  if (basename !== 'README.md' && !/^[a-z0-9]+(?:-[a-z0-9]+)*\.md$/.test(basename)) {
    fail(file, 'filename must use lowercase kebab-case')
  }

  const markdown = readFileSync(file, 'utf8')
  const metadata = parseFrontMatter(file, markdown)
  if (metadata?.status === 'current') currentDocuments.push({ file, markdown })

  const withoutFences = stripFencedCode(markdown)
  const h1Count = (withoutFences.match(/^# /gm) ?? []).length
  if (h1Count !== 1) fail(file, `expected exactly one H1 heading, found ${h1Count}`)
}

for (const file of markdownFiles) {
  const markdown = stripFencedCode(readFileSync(file, 'utf8'))
  const prose = markdown.replace(/`[^`\n]+`/g, '')
  if (/\bmy(?:\s+)?copilot(?:\s+next)?\b/i.test(prose)) {
    fail(file, 'obsolete product name in prose; use “Captain Who”')
  }
  for (const match of markdown.matchAll(/\]\(([^)]+)\)/g)) {
    const target = resolveMarkdownTarget(file, match[1])
    if (target && !existsSync(target)) fail(file, `local link does not exist: ${match[1]}`)
    if (file === path.join(docsRoot, 'README.md') && target) indexedTargets.add(target)
  }

  for (const match of markdown.matchAll(/`([^`\n]+)`/g)) {
    let candidate = match[1].trim()
    const repositoryPath = /^(?:src|crates|packages|scripts|resources|build|docs)\//.test(candidate)
    const rootFile = new Set([
      '.node-version',
      'Cargo.lock',
      'Cargo.toml',
      'electron-builder.yml',
      'package.json',
      'pnpm-lock.yaml',
      'THIRD_PARTY_GRAMMAR_NOTICES.txt',
      'THIRD_PARTY_NOTICES.txt',
      'vitest.config.ts'
    ]).has(candidate.split('#')[0])
    if ((!repositoryPath && !rootFile) || /[ *?{}<>|]/.test(candidate)) continue
    candidate = candidate
      .split('#')[0]
      .replace(/:\d+(?::\d+)?$/, '')
      .replace(/[.,;:]$/, '')
    if (!existsSync(path.join(repositoryRoot, candidate))) {
      fail(file, `repository path in code span does not exist: ${match[1]}`)
    }
  }

  for (const match of markdown.matchAll(/\bpnpm\s+([a-zA-Z0-9:_<>.-]+)/g)) {
    const command = match[1]
    if (
      ['exec', 'install', 'lock', 'run', '<command>'].includes(command) ||
      /^\d/.test(command) ||
      command.includes('<')
    ) {
      continue
    }
    if (!packageJson.scripts[command]) fail(file, `unknown pnpm script: ${command}`)
  }
}

for (const file of documentationFiles) {
  if (file !== path.join(docsRoot, 'README.md') && !indexedTargets.has(file)) {
    fail(file, 'not linked from docs/README.md')
  }
}

for (const legacyPath of [
  'architecture.md',
  'context-management.md',
  'mcp.md',
  'multi-agent-architecture.md',
  'multi-agent-release-gate.md',
  'right-sidebar-platform.md',
  'tool-result-consumer-matrix.md',
  'tool-result-limits.md'
]) {
  const file = path.join(docsRoot, legacyPath)
  if (existsSync(file)) fail(file, 'legacy top-level document must not be restored')
}

const migrations = readFileSync(
  path.join(repositoryRoot, 'crates/core/src/storage/migrations.rs'),
  'utf8'
)
const storageVersion = /STORAGE_SCHEMA_VERSION:\s*i32\s*=\s*(\d+)/.exec(migrations)?.[1]
if (!storageVersion) {
  failures.push('crates/core/src/storage/migrations.rs: could not read STORAGE_SCHEMA_VERSION')
} else {
  for (const authoritativePath of [
    'docs/architecture/overview.md',
    'docs/architecture/storage-and-data-lifecycle.md',
    'docs/architecture/multi-agent.md',
    'docs/operations/multi-agent-release-gate.md'
  ]) {
    const file = path.join(repositoryRoot, authoritativePath)
    if (!new RegExp(`\\bv${storageVersion}\\b`).test(readFileSync(file, 'utf8'))) {
      fail(file, `must name current storage schema v${storageVersion}`)
    }
  }

  const releaseGateRunner = path.join(repositoryRoot, 'scripts/run-multi-agent-release-gate.mjs')
  const releaseGateRunnerSource = readFileSync(releaseGateRunner, 'utf8')
  if (!releaseGateRunnerSource.includes(`storage: canonical v${storageVersion},`)) {
    fail(releaseGateRunner, `storage step label must match canonical schema v${storageVersion}`)
  }
}

const pnpmVersion = String(packageJson.packageManager ?? '').replace(/^pnpm@/, '')
const nodeVersion = readFileSync(path.join(repositoryRoot, '.node-version'), 'utf8').trim()
for (const [documentPath, expected] of [
  ['README.md', `pnpm ${pnpmVersion}`],
  ['README.md', `Node.js ${nodeVersion}`],
  ['docs/development/getting-started.md', `pnpm ${pnpmVersion}`],
  ['docs/development/getting-started.md', `Node.js ${nodeVersion}`]
]) {
  const file = path.join(repositoryRoot, documentPath)
  if (!readFileSync(file, 'utf8').includes(expected))
    fail(file, `missing version fact: ${expected}`)
}

const runtimeComponents = path.join(docsRoot, 'development/runtime-components.md')
const runtimeComponentsMarkdown = readFileSync(runtimeComponents, 'utf8')
for (const dependency of ['@modelcontextprotocol/sdk', '@playwright/mcp', 'playwright']) {
  const version = packageJson.dependencies[dependency]
  if (!version || !runtimeComponentsMarkdown.includes(version)) {
    fail(
      runtimeComponents,
      `missing package.json version for ${dependency}: ${version ?? 'not found'}`
    )
  }
}

const officeCliManifest = readJson('resources/officecli-manifest.json')
const officeRendererManifest = readJson('resources/office-renderer-manifest.json')
const wordPdfManifest = readJson('resources/word-pdf-renderer-manifest.json')
const artifactRuntimeManifest = readJson('resources/artifact-runtime-manifest.json')
const playwrightConformance = readJson(
  'crates/core-server/resources/playwright-conformance-report.json'
)
const runtimeFacts = [
  ['OfficeCLI', officeCliManifest.component.version],
  ['Office renderer bundle', officeRendererManifest.bundleVersion],
  ['Office renderer Chromium', officeRendererManifest.browser.version],
  ['Office renderer revision', officeRendererManifest.browser.revision],
  ['Office renderer Playwright', officeRendererManifest.browser.playwrightVersion],
  ['Word/PDF renderer bundle', wordPdfManifest.bundleVersion],
  ['LibreOffice', wordPdfManifest.libreOffice.version],
  ['Artifact Runtime bundle', artifactRuntimeManifest.bundleVersion],
  ['Artifact managed Node', artifactRuntimeManifest.node.version],
  [
    'Artifact managed Python',
    `${artifactRuntimeManifest.python.version}+${artifactRuntimeManifest.python.release}`
  ],
  ['Artifact ripgrep', artifactRuntimeManifest.tools.ripgrep.version]
]
for (const [component, version] of runtimeFacts) {
  if (!runtimeComponentsMarkdown.includes(String(version))) {
    fail(runtimeComponents, `missing manifest version for ${component}: ${version}`)
  }
}

for (const [runtime, dependencies] of [
  ['Node', artifactRuntimeManifest.node.dependencies],
  ['Python', artifactRuntimeManifest.python.dependencies]
]) {
  for (const dependency of dependencies) {
    const expected = `${dependency.name}@${dependency.version}`
    if (!runtimeComponentsMarkdown.includes(expected)) {
      fail(runtimeComponents, `missing Artifact Runtime ${runtime} dependency: ${expected}`)
    }
  }
}

for (const documentPath of [
  'docs/subsystems/mcp.md',
  'docs/subsystems/browser-automation.md',
  'docs/development/runtime-components.md'
]) {
  const file = path.join(repositoryRoot, documentPath)
  const markdown = readFileSync(file, 'utf8')
  for (const [classification, count] of [
    ['upstream', playwrightConformance.classification.upstreamToolCount],
    ['exposed', playwrightConformance.classification.exposedToolCount]
  ]) {
    if (!new RegExp(`\\b${count}\\b`).test(markdown)) {
      fail(file, `must name Managed Playwright ${classification} Tool count ${count}`)
    }
  }
}

const managedPlaywrightBridgeSource = readFileSync(
  path.join(repositoryRoot, 'packages/protocol/src/mcp/managedPlaywrightBridge.ts'),
  'utf8'
)
const managedPlaywrightBridgeVersion = /MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION\s*=\s*(\d+)/.exec(
  managedPlaywrightBridgeSource
)?.[1]
const mcpDocument = path.join(docsRoot, 'subsystems/mcp.md')
if (
  !managedPlaywrightBridgeVersion ||
  !new RegExp(`\\bschema v${managedPlaywrightBridgeVersion}\\b`).test(
    readFileSync(mcpDocument, 'utf8')
  )
) {
  fail(
    mcpDocument,
    `must match Managed Playwright bridge schema v${managedPlaywrightBridgeVersion ?? 'unknown'}`
  )
}

const automationProtocolSource = readFileSync(
  path.join(repositoryRoot, 'packages/protocol/src/automations.ts'),
  'utf8'
)
const automationSchemaVersion = /AUTOMATION_SCHEMA_VERSION\s*=\s*(\d+)/.exec(
  automationProtocolSource
)?.[1]
const automationPermissionModeVersion = /AUTOMATION_PERMISSION_MODE_VERSION\s*=\s*(\d+)/.exec(
  automationProtocolSource
)?.[1]
const automationErrorCode = /AUTOMATION_ERROR_CODE\s*=\s*(-?\d+)/.exec(
  automationProtocolSource
)?.[1]
const automationRustProtocolSource = readFileSync(
  path.join(repositoryRoot, 'crates/protocol-rs/src/automations.rs'),
  'utf8'
)
const rustAutomationFacts = {
  schema: /AUTOMATION_SCHEMA_VERSION:\s*u32\s*=\s*(\d+)/.exec(automationRustProtocolSource)?.[1],
  permission: /AUTOMATION_PERMISSION_MODE_VERSION:\s*u32\s*=\s*(\d+)/.exec(
    automationRustProtocolSource
  )?.[1],
  error: /AUTOMATION_ERROR_CODE:\s*i32\s*=\s*(-?\d+)/.exec(automationRustProtocolSource)?.[1]
}
const automationDocument = path.join(docsRoot, 'subsystems/scheduled-automations.md')
if (!automationSchemaVersion || !automationPermissionModeVersion || !automationErrorCode) {
  failures.push('packages/protocol/src/automations.ts: could not read Automation protocol versions')
} else {
  const automationMarkdown = readFileSync(automationDocument, 'utf8')
  for (const expected of [
    `Automation DTO`,
    `schemaVersion\` 各为 **v${automationSchemaVersion}**`,
    `permission mode v${automationPermissionModeVersion}`,
    `automation-contract-v${automationSchemaVersion}.json`,
    `\`${automationErrorCode}\``
  ]) {
    if (!automationMarkdown.includes(expected)) {
      fail(automationDocument, `missing Automation protocol fact: ${expected}`)
    }
  }
  for (const [fact, typescriptValue, rustValue] of [
    ['schema version', automationSchemaVersion, rustAutomationFacts.schema],
    ['permission mode version', automationPermissionModeVersion, rustAutomationFacts.permission],
    ['error code', automationErrorCode, rustAutomationFacts.error]
  ]) {
    if (rustValue !== typescriptValue) {
      failures.push(
        `crates/protocol-rs/src/automations.rs: Automation ${fact} ${rustValue ?? 'unknown'} does not match TypeScript ${typescriptValue}`
      )
    }
  }

  const automationIpcDocument = path.join(docsRoot, 'architecture/ipc-and-protocol.md')
  const automationIpcMarkdown = readFileSync(automationIpcDocument, 'utf8')
  for (const expected of [
    `AUTOMATION_SCHEMA_VERSION = ${automationSchemaVersion}`,
    `AUTOMATION_PERMISSION_MODE_VERSION = ${automationPermissionModeVersion}`,
    `\`${automationErrorCode}\``
  ]) {
    if (!automationIpcMarkdown.includes(expected)) {
      fail(automationIpcDocument, `missing Automation protocol fact: ${expected}`)
    }
  }
}

const protocolVersionGroups = [
  {
    label: 'Notification schema',
    sources: [
      ['packages/protocol/src/notifications.ts', /NOTIFICATION_SCHEMA_VERSION\s*=\s*(\d+)/],
      ['crates/protocol-rs/src/notifications.rs', /NOTIFICATION_SCHEMA_VERSION:\s*u32\s*=\s*(\d+)/]
    ],
    documents: [
      ['docs/subsystems/notifications.md', (version) => `Notification schema v${version}`],
      [
        'docs/architecture/ipc-and-protocol.md',
        (version) => `NOTIFICATION_SCHEMA_VERSION = ${version}`
      ]
    ]
  },
  {
    label: 'FileChange schema',
    sources: [
      ['packages/protocol/src/agent.ts', /AGENT_FILE_CHANGE_SCHEMA_VERSION\s*=\s*(\d+)/],
      [
        'crates/core/src/protocol.rs',
        /AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION:\s*u32\s*=\s*(\d+)/
      ],
      ['crates/core/src/file_change/model.rs', /FILE_CHANGE_SCHEMA_VERSION:\s*u32\s*=\s*(\d+)/]
    ],
    documents: [
      ['docs/subsystems/file-change.md', (version) => `FileChange schema v${version}`],
      ['docs/architecture/ipc-and-protocol.md', (version) => `\`FileChange\` 使用 v${version} 协议`]
    ]
  },
  {
    label: 'Browser data schema',
    sources: [
      ['packages/protocol/src/browserData.ts', /BROWSER_DATA_SCHEMA_VERSION\s*=\s*(\d+)/],
      ['crates/core/src/storage/models.rs', /BROWSER_DATA_SCHEMA_VERSION:\s*u32\s*=\s*(\d+)/]
    ],
    documents: [
      [
        'docs/subsystems/browser-automation.md',
        (version) => `Browser data schema 当前为 v${version}`
      ],
      ['docs/architecture/ipc-and-protocol.md', (version) => `data/preferences/history v${version}`]
    ]
  },
  {
    label: 'Browser download schema',
    sources: [
      ['packages/protocol/src/browserDownloads.ts', /BROWSER_DOWNLOAD_SCHEMA_VERSION\s*=\s*(\d+)/],
      ['crates/core/src/storage/models.rs', /BROWSER_DOWNLOAD_SCHEMA_VERSION:\s*u32\s*=\s*(\d+)/]
    ],
    documents: [
      ['docs/subsystems/browser-automation.md', (version) => `协议 v${version} 的 durable 引用`],
      ['docs/architecture/ipc-and-protocol.md', (version) => `download v${version}`]
    ]
  }
]

for (const group of protocolVersionGroups) {
  const versions = group.sources.map(([repositoryPath, pattern]) => {
    const file = path.join(repositoryRoot, repositoryPath)
    const version = pattern.exec(readFileSync(file, 'utf8'))?.[1]
    if (!version) fail(file, `could not read ${group.label}`)
    return { repositoryPath, version }
  })
  const expectedVersion = versions.find(({ version }) => version)?.version
  if (!expectedVersion) continue

  for (const { repositoryPath, version } of versions) {
    if (version && version !== expectedVersion) {
      failures.push(
        `${repositoryPath}: ${group.label} ${version} does not match ${expectedVersion}`
      )
    }
  }

  for (const [documentPath, expectedFact] of group.documents) {
    const file = path.join(repositoryRoot, documentPath)
    const expected = expectedFact(expectedVersion)
    if (!readFileSync(file, 'utf8').includes(expected)) {
      fail(file, `missing ${group.label} fact: ${expected}`)
    }
  }
}

const automationE2eScript = 'test:automation-core-e2e'
if (!packageJson.scripts[automationE2eScript]) {
  failures.push(`package.json: missing pnpm script ${automationE2eScript}`)
} else {
  for (const documentPath of [
    'README.md',
    'docs/subsystems/scheduled-automations.md',
    'docs/development/testing.md'
  ]) {
    const file = path.join(repositoryRoot, documentPath)
    if (!readFileSync(file, 'utf8').includes(`pnpm ${automationE2eScript}`)) {
      fail(file, `missing Automation E2E command: pnpm ${automationE2eScript}`)
    }
  }
}

const forbiddenCurrentClaims = [
  ['crates/core-server/src/agent.rs', 'removed Core Server path'],
  ['当前项目没有配置代码签名', 'obsolete code-signing statement'],
  ['notifications are `collaboration.', 'obsolete collaboration notification namespace'],
  ['`notification_facts`', 'nonexistent notification table; use notification_events']
]
for (const { file, markdown } of currentDocuments) {
  for (const [needle, description] of forbiddenCurrentClaims) {
    if (markdown.includes(needle)) fail(file, description)
  }

  if (file !== path.join(docsRoot, 'development/glossary.md')) {
    const prose = stripFencedCode(markdown).replace(/`[^`\n]+`/g, '')
    for (const [needle, preferred] of [
      ['managed Playwright', 'Managed Playwright'],
      ['Rust 核心', 'Rust Core'],
      ['Rust Core sidecar', 'Core Server sidecar'],
      ['Rust Core Server', 'Core Server'],
      ['外部 stdio MCP', '用户配置的 stdio MCP Server'],
      ['子智能体', '子 Agent'],
      ['主智能体', '根 Agent'],
      ['从智能体', '子 Agent']
    ]) {
      if (prose.includes(needle)) fail(file, `use canonical term “${preferred}”, found “${needle}”`)
    }

    const withoutCanonicalCoreNames = prose
      .replaceAll('Core Server', '')
      .replaceAll('Rust Core', '')
      .replaceAll('Playwright Core', '')
    if (/\bCore\b/.test(withoutCanonicalCoreNames)) {
      fail(file, 'ambiguous standalone “Core”; use “Core Server” or “Rust Core”')
    }
  }
}

if (failures.length > 0) {
  console.error(`Documentation checks failed (${failures.length}):`)
  for (const failure of failures) console.error(`- ${failure}`)
  process.exit(1)
}

console.log(
  `Documentation checks passed: ${documentationFiles.length} docs, ${markdownFiles.length} Markdown files, storage schema v${storageVersion}.`
)
