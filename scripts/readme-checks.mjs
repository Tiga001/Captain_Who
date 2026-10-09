/* eslint-disable @typescript-eslint/explicit-function-return-type -- Documentation helpers are covered by runtime tests. */
import path from 'node:path'

export function withoutCode(markdown) {
  let fence = null
  return markdown
    .replace(/<!--[\s\S]*?-->/g, '')
    .split('\n')
    .map((line) => {
      const marker = /^ {0,3}(`{3,}|~{3,})/.exec(line)?.[1]
      if (fence) {
        if (marker?.[0] === fence[0] && marker.length >= fence.length) fence = null
        return ''
      }
      if (marker) {
        fence = marker
        return ''
      }
      return line
    })
    .join('\n')
}

function decodeEntities(value) {
  return value.replace(/&(#x[\da-f]+|#\d+|amp|quot|apos|lt|gt);/gi, (entity, name) => {
    if (name.startsWith('#')) {
      const code =
        name[1].toLowerCase() === 'x' ? parseInt(name.slice(2), 16) : Number(name.slice(1))
      return code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : entity
    }
    return { amp: '&', quot: '"', apos: "'", lt: '<', gt: '>' }[name.toLowerCase()]
  })
}

function attributes(tag) {
  return [...tag.matchAll(/\b([\w-]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/g)].map(
    ([, name, doubleQuoted, singleQuoted, unquoted]) => [
      name.toLowerCase(),
      decodeEntities(doubleQuoted ?? singleQuoted ?? unquoted)
    ]
  )
}

function headingText(value) {
  return decodeEntities(value.replace(/<[^>]*>/g, '').replace(/!?\[([^\]]+)\]\([^)]*\)/g, '$1'))
    .replace(/[`*~]/g, '')
    .replace(/(^|\s)_+(.+?)_+(?=$|\s)/g, '$1$2')
    .replace(/\s+/g, ' ')
    .trim()
}

export function readmeStructure(markdown) {
  const source = withoutCode(markdown)
  const headings = []
  for (const match of source.matchAll(
    /^ {0,3}(#{1,6})\s+(.+?)(?:\s+#+)?\s*$|<h([1-6])\b[^>]*>([\s\S]*?)<\/h\3\s*>/gim
  )) {
    headings.push({
      depth: Number(match[3] ?? match[1].length),
      text: headingText(match[4] ?? match[2]),
      index: match.index
    })
  }
  for (const match of source.matchAll(/^([^\n]+)\n {0,3}(=+|-+)\s*$/gm)) {
    if (!/^\s*[#<>|]/.test(match[1])) {
      headings.push({
        depth: match[2][0] === '=' ? 1 : 2,
        text: headingText(match[1]),
        index: match.index
      })
    }
  }
  headings.sort((left, right) => left.index - right.index)
  const anchors = new Map()
  const usedSlugs = new Set()
  for (const [index, heading] of headings.entries()) {
    // GitHub-style heading slugs retain Unicode letters, numbers, marks, hyphens and underscores.
    const base = heading.text
      .toLowerCase()
      .replace(/[^\p{L}\p{N}\p{M}_\-\s]/gu, '')
      .replace(/\s/g, '-')
    let slug = base
    for (let suffix = 1; usedSlugs.has(slug); suffix += 1) slug = `${base}-${suffix}`
    usedSlugs.add(slug)
    anchors.set(slug, `heading-${index}`)
  }
  for (const tag of source.matchAll(/<[a-z][^>]*>/gi)) {
    for (const [name, value] of attributes(tag[0])) {
      if (name === 'id' || (name === 'name' && /^<a\b/i.test(tag[0]))) {
        anchors.set(value, value)
      }
    }
  }
  return { headings, anchors }
}

function destination(source) {
  const angle = /^<([^>]+)>/.exec(source)
  if (angle) return angle[1]
  let depth = 0
  let value = ''
  for (let index = 0; index < source.length; index += 1) {
    const character = source[index]
    if (character === '\\' && index + 1 < source.length) {
      value += source[++index]
      continue
    }
    if (character === ')' && depth === 0) break
    if (/\s/.test(character) && depth === 0) break
    if (character === '(') depth += 1
    if (character === ')') depth -= 1
    value += character
  }
  return value
}

export function readmeReferences(markdown) {
  const source = withoutCode(markdown).replace(/(`+)[\s\S]*?\1/g, '')
  const references = []
  for (const match of source.matchAll(/\]\(/g)) {
    // Walk back over nested labels so linked images contribute both targets.
    let depth = 1
    let start = match.index - 1
    for (; start >= 0; start -= 1) {
      if (source[start - 1] === '\\') continue
      if (source[start] === ']') depth += 1
      if (source[start] === '[') depth -= 1
      if (depth === 0) break
    }
    if (start < 0) continue
    const target = destination(source.slice(match.index + 2).trimStart())
    if (target) references.push({ kind: source[start - 1] === '!' ? 'image' : 'link', target })
  }
  const definitions = new Map()
  const labelKey = (label) => label.trim().replace(/\s+/g, ' ').toLowerCase()
  const withoutDefinitions = source.replace(
    /^ {0,3}\[([^\]]+)\]:\s*(.+)$/gm,
    (_, label, target) => {
      definitions.set(labelKey(label), destination(target))
      return ''
    }
  )
  for (const match of withoutDefinitions.matchAll(/(!?)\[([^\]\n]+)\](?:\[([^\]\n]*)\])?(?!\()/g)) {
    const target = definitions.get(labelKey(match[3] || match[2]))
    if (target) references.push({ kind: match[1] ? 'image' : 'link', target })
  }
  for (const match of source.matchAll(/<[a-z][^>]*>/gi)) {
    for (const [name, value] of attributes(match[0])) {
      if (name === 'href') references.push({ kind: 'link', target: value })
      if (name === 'src') references.push({ kind: 'image', target: value })
      if (name === 'srcset') {
        // URLs end at whitespace, not embedded commas (for example in data URLs).
        let remaining = value
        while (remaining) {
          remaining = remaining.replace(/^[,\s]+/, '')
          const candidate = /^\S+/.exec(remaining)?.[0]
          if (!candidate) break
          references.push({ kind: 'image', target: candidate.replace(/,+$/, '') })
          remaining = remaining.slice(candidate.length)
          if (!candidate.endsWith(',')) {
            const separator = remaining.indexOf(',')
            remaining = separator < 0 ? '' : remaining.slice(separator + 1)
          }
        }
      }
    }
  }
  return references.map((reference) => ({ ...reference, target: decodeEntities(reference.target) }))
}

export function localReference(file, target, repositoryRoot) {
  if (/^(?:[a-z][a-z\d+.-]*:|\/\/)/i.test(target)) return null
  const hash = target.indexOf('#')
  const beforeHash = hash < 0 ? target : target.slice(0, hash)
  const pathname = decodeURIComponent(beforeHash.split('?')[0])
  return {
    file: pathname
      ? path.resolve(
          pathname.startsWith('/') ? repositoryRoot : path.dirname(file),
          pathname.replace(/^\//, '')
        )
      : file,
    fragment: hash < 0 ? '' : decodeURIComponent(target.slice(hash + 1))
  }
}

export function readmeParity(left, right, repositoryRoot) {
  const errors = []
  const structures = [left, right].map(({ markdown }) => readmeStructure(markdown))
  const depths = structures.map(({ headings }) => headings.map(({ depth }) => depth).join(','))
  if (depths[0] !== depths[1]) errors.push('bilingual README heading depth sequences differ')
  const readmes = new Set(
    ['README.md', 'README.en.md', 'README.zh-CN.md'].map((file) => path.join(repositoryRoot, file))
  )
  const structuresByFile = new Map([
    [left.file, structures[0]],
    [right.file, structures[1]],
    [path.join(repositoryRoot, 'README.en.md'), structures[0]]
  ])
  function targets(document, kind) {
    return new Set(
      readmeReferences(document.markdown)
        .filter((reference) => reference.kind === kind)
        .map(({ target }) => {
          let local
          try {
            local = localReference(document.file, target, repositoryRoot)
          } catch {
            return target // The link validator reports malformed percent-encoding separately.
          }
          if (!local) return target
          const name = readmes.has(local.file)
            ? '@readme'
            : path.relative(repositoryRoot, local.file).split(path.sep).join('/')
          const fragment =
            structuresByFile.get(local.file)?.anchors.get(local.fragment) ?? local.fragment
          return `${name}${fragment ? `#${fragment}` : ''}`
        })
    )
  }
  for (const kind of ['image', 'link']) {
    const [leftTargets, rightTargets] = [left, right].map((document) => targets(document, kind))
    for (const [present, absent, document] of [
      [leftTargets, rightTargets, right],
      [rightTargets, leftTargets, left]
    ]) {
      const missing = [...present].filter((target) => !absent.has(target)).sort()
      if (missing.length)
        errors.push(
          `${path.basename(document.file)} is missing bilingual ${kind} targets: ${missing.join(', ')}`
        )
    }
  }
  return errors
}
