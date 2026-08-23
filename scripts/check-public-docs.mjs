/* eslint-disable @typescript-eslint/explicit-function-return-type -- Public documentation checker is runtime-validated JavaScript. */
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const repositoryRoot = path.resolve(fileURLToPath(new URL('..', import.meta.url)))
const publicDocsRoot = path.join(repositoryRoot, 'public-docs')
const internalDocsRoot = path.join(repositoryRoot, 'docs')
const failures = []

const requiredDocuments = [
  'README.md',
  'user/README.md',
  'user/getting-started/README.md',
  'user/getting-started/product-overview.md',
  'user/getting-started/installation.md',
  'user/getting-started/interface-tour.md',
  'user/getting-started/first-task.md',
  'user/getting-started/first-project.md',
  'user/everyday-use/README.md',
  'user/everyday-use/tasks-and-conversations.md',
  'user/everyday-use/files-and-git.md',
  'user/everyday-use/search-and-browser.md',
  'user/everyday-use/models-and-providers.md',
  'user/everyday-use/permissions-and-approvals.md',
  'user/everyday-use/context-and-history.md',
  'user/capabilities/README.md',
  'user/capabilities/tools.md',
  'user/capabilities/skills.md',
  'user/capabilities/mcp.md',
  'user/capabilities/multi-agent.md',
  'user/capabilities/automations.md',
  'user/capabilities/browser-automation.md',
  'user/capabilities/artifacts-and-office.md',
  'user/tutorials/README.md',
  'user/tutorials/understand-a-project.md',
  'user/tutorials/implement-a-feature.md',
  'user/tutorials/research-and-write.md',
  'user/tutorials/use-multiple-agents.md',
  'user/tutorials/connect-an-mcp-server.md',
  'user/tutorials/create-a-skill.md',
  'user/tutorials/create-an-automation.md',
  'user/learn/README.md',
  'user/learn/from-chatbot-to-agent.md',
  'user/learn/agent-loop.md',
  'user/learn/tool-calling.md',
  'user/learn/context-management.md',
  'user/learn/multi-agent-principles.md',
  'user/learn/skills-and-mcp.md',
  'user/learn/automation-principles.md',
  'user/learn/system-overview.md',
  'user/best-practices/README.md',
  'user/best-practices/writing-effective-tasks.md',
  'user/best-practices/managing-large-projects.md',
  'user/best-practices/choosing-capabilities.md',
  'user/best-practices/safe-agent-usage.md',
  'user/reference/README.md',
  'user/reference/settings.md',
  'user/reference/statuses.md',
  'user/reference/keyboard-shortcuts.md',
  'user/reference/capability-limits.md',
  'user/reference/glossary.md',
  'integrations/README.md',
  'integrations/overview.md',
  'integrations/skill-development.md',
  'integrations/mcp-server-integration.md',
  'integrations/model-provider-integration.md',
  'integrations/compatibility.md',
  'releases/README.md',
  'releases/supported-platforms.md',
  'releases/upgrade-guide.md',
  'support/README.md',
  'support/faq.md',
  'support/common-problems.md',
  'support/diagnostics-and-logs.md',
  'support/known-issues.md',
  'security/README.md',
  'security/security-overview.md',
  'security/data-and-permissions.md',
  'security/safe-use.md',
  'legal/README.md',
  'legal/third-party-software.md'
]

const forbiddenPlaceholderDocuments = [
  'legal/privacy-policy.md',
  'legal/terms-of-use.md',
  'legal/trademarks.md',
  'releases/release-notes/README.md',
  'releases/version-lifecycle.md',
  'security/security-updates.md',
  'security/vulnerability-reporting.md',
  'support/getting-help.md',
  'support/report-a-problem.md'
]

const allowedExternalFiles = new Set([
  path.join(repositoryRoot, 'THIRD_PARTY_NOTICES.txt'),
  path.join(repositoryRoot, 'THIRD_PARTY_GRAMMAR_NOTICES.txt')
])

function walkMarkdown(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const absolute = path.join(directory, entry.name)
    return entry.isDirectory()
      ? walkMarkdown(absolute)
      : entry.isFile() && entry.name.endsWith('.md')
        ? [absolute]
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
  for (const key of ['title', 'description', 'status', 'audience', 'owner', 'last_verified']) {
    if (!metadata[key]) fail(file, `missing front matter field ${key}`)
  }
  if (metadata.status && metadata.status !== 'current') {
    fail(file, `public documents must use status current, found ${metadata.status}`)
  }
  if (
    metadata.audience &&
    !['public', 'user', 'integration-developer'].includes(metadata.audience)
  ) {
    fail(file, `unsupported public audience ${metadata.audience}`)
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

if (!existsSync(publicDocsRoot)) {
  console.error('public-docs: directory does not exist')
  process.exit(1)
}

for (const required of requiredDocuments) {
  const file = path.join(publicDocsRoot, required)
  if (!existsSync(file))
    failures.push(`public-docs/${required}: required public document is missing`)
}

for (const forbidden of forbiddenPlaceholderDocuments) {
  const file = path.join(publicDocsRoot, forbidden)
  if (existsSync(file)) {
    failures.push(`public-docs/${forbidden}: unverified placeholder document must not be published`)
  }
}

const documentationFiles = walkMarkdown(publicDocsRoot).sort()
const indexedTargets = new Set()

for (const file of documentationFiles) {
  const basename = path.basename(file)
  if (basename !== 'README.md' && !/^[a-z0-9]+(?:-[a-z0-9]+)*\.md$/.test(basename)) {
    fail(file, 'filename must use lowercase kebab-case')
  }

  const markdown = readFileSync(file, 'utf8')
  parseFrontMatter(file, markdown)
  const withoutFences = stripFencedCode(markdown)
  const h1Count = (withoutFences.match(/^# /gm) ?? []).length
  if (h1Count !== 1) fail(file, `expected exactly one H1 heading, found ${h1Count}`)

  for (const match of withoutFences.matchAll(/\]\(([^)]+)\)/g)) {
    const target = resolveMarkdownTarget(file, match[1])
    if (!target) continue
    if (!existsSync(target)) {
      fail(file, `local link does not exist: ${match[1]}`)
      continue
    }
    if (target === internalDocsRoot || target.startsWith(`${internalDocsRoot}${path.sep}`)) {
      fail(file, `public document links to internal docs: ${match[1]}`)
    }
    if (
      target !== publicDocsRoot &&
      !target.startsWith(`${publicDocsRoot}${path.sep}`) &&
      !allowedExternalFiles.has(target)
    ) {
      fail(file, `local link escapes the public documentation boundary: ${match[1]}`)
    }
    if (basename === 'README.md' && target.endsWith('.md')) indexedTargets.add(target)
  }
}

for (const file of documentationFiles) {
  if (file === path.join(publicDocsRoot, 'README.md')) continue
  if (!indexedTargets.has(file)) fail(file, 'document is not linked from a README index')
}

const thirdPartyPage = path.join(publicDocsRoot, 'legal/third-party-software.md')
if (existsSync(thirdPartyPage)) {
  const markdown = readFileSync(thirdPartyPage, 'utf8')
  for (const notice of ['THIRD_PARTY_NOTICES.txt', 'THIRD_PARTY_GRAMMAR_NOTICES.txt']) {
    if (!markdown.includes(notice)) fail(thirdPartyPage, `missing link to ${notice}`)
  }
}

if (failures.length > 0) {
  console.error(`Public documentation checks failed (${failures.length}):`)
  for (const failure of failures) console.error(`- ${failure}`)
  process.exit(1)
}

console.log(`Public documentation checks passed: ${documentationFiles.length} Markdown files.`)
