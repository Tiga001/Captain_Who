import assert from 'node:assert/strict'
import test from 'node:test'
import {
  localReference,
  readmeParity,
  readmeReferences,
  readmeStructure
} from './readme-checks.mjs'

test('recognizes semantic Markdown and HTML product headings, excluding examples', () => {
  for (const heading of [
    '# Captain Who',
    '<h1 align="center">Captain Who</h1>',
    'Captain Who\n==========='
  ]) {
    const { headings } = readmeStructure(`${heading}\n\n~~~md\n# Fake\n~~~\n<!-- # Fake -->`)
    assert.deepEqual(
      headings.map(({ depth, text }) => ({ depth, text })),
      [{ depth: 1, text: 'Captain Who' }]
    )
  }
  assert.deepEqual(
    readmeStructure('## Captain Who\n\nNot a # Captain Who heading').headings.map(
      ({ depth }) => depth
    ),
    [2]
  )
})

test('extracts Markdown, HTML, reference links and both linked badge targets', () => {
  const markdown = `
[![badge](https://img.shields.io/badge/License-Apache-blue)](LICENSE)
![screen](assets/screen(1).png "A screen")
[guide](<docs/guide with spaces.md#first> "Guide")
<picture><source srcset="assets/dark.png 1x, assets/dark@2x.png 2x"><img src='assets/light.png'></picture>
<a href="docs/guide.md#hello-world">Guide</a>
[reference][guide]
![image][screen]
[guide]: docs/guide.md
[screen]: assets/reference.png
\`[ignored](missing.md)\`
<!-- <img src="missing.png"> -->
\`\`\`md
[ignored](missing.md)
\`\`\`
`
  const references = readmeReferences(markdown)
  assert.deepEqual(
    references
      .filter(({ kind }) => kind === 'image')
      .map(({ target }) => target)
      .sort(),
    [
      'assets/dark.png',
      'assets/dark@2x.png',
      'assets/light.png',
      'assets/reference.png',
      'assets/screen(1).png',
      'https://img.shields.io/badge/License-Apache-blue'
    ]
  )
  assert.deepEqual(
    references
      .filter(({ kind }) => kind === 'link')
      .map(({ target }) => target)
      .sort(),
    ['LICENSE', 'docs/guide with spaces.md#first', 'docs/guide.md', 'docs/guide.md#hello-world']
  )
})

test('resolves local destinations, decoded fragments, queries and protocol-relative URLs', () => {
  assert.deepEqual(
    localReference('/repo/README.md', 'docs/guide%20one.md?view=1#%E4%B8%AD%E6%96%87', '/repo'),
    {
      file: '/repo/docs/guide one.md',
      fragment: '中文'
    }
  )
  assert.deepEqual(localReference('/repo/README.md', '#quick-start', '/repo'), {
    file: '/repo/README.md',
    fragment: 'quick-start'
  })
  assert.deepEqual(localReference('/repo/docs/README.md', '/LICENSE', '/repo'), {
    file: '/repo/LICENSE',
    fragment: ''
  })
  for (const target of [
    'https://example.com',
    '//example.com/image.png',
    'mailto:help@example.com',
    'data:image/png;base64,test'
  ]) {
    assert.equal(localReference('/repo/README.md', target, '/repo'), null)
  }
  assert.throws(() => localReference('/repo/README.md', 'bad%ZZ.md', '/repo'), URIError)
})

test('srcset parsing does not mistake embedded data URL commas for local paths', () => {
  assert.deepEqual(
    readmeReferences('<img srcset="data:image/png;base64,AAAA 1x, assets/large.png 2x">'),
    [
      { kind: 'image', target: 'data:image/png;base64,AAAA' },
      { kind: 'image', target: 'assets/large.png' }
    ]
  )
  assert.deepEqual(
    readmeReferences('<source srcset="assets/first.png, assets/second.png">').map(
      ({ target }) => target
    ),
    ['assets/first.png', 'assets/second.png']
  )
})

test('tracks Unicode and duplicate heading fragments plus explicit HTML anchors', () => {
  const { anchors } = readmeStructure(
    '# Captain Who\n## Hello, **World**!\n## Hello, World!\n## 快速开始\n## `schema_version`\n<a id="explicit"></a>\n<a name="legacy"></a>'
  )
  assert.deepEqual(
    [...anchors.keys()],
    [
      'captain-who',
      'hello-world',
      'hello-world-1',
      '快速开始',
      'schema_version',
      'explicit',
      'legacy'
    ]
  )
  assert.equal(anchors.has('missing-fragment'), false)
})

const english = {
  file: '/repo/README.md',
  markdown:
    '<h1 align="center">Captain Who</h1>\n[中文](README.zh-CN.md)\n[Start](#quick-start)\n## Quick start\n![Screen](assets/screen.png)\n[Guide](docs/guide.md#installation)'
}
const chinese = {
  file: '/repo/README.zh-CN.md',
  markdown:
    '<h1 align="center">Captain Who</h1>\n[English](README.md)\n[开始](#快速开始)\n## 快速开始\n![截图](assets/screen.png)\n[指南](docs/guide.md#installation)'
}

test('bilingual parity allows translated labels, headings, navigation and README aliases', () => {
  assert.deepEqual(readmeParity(english, chinese, '/repo'), [])
  assert.deepEqual(
    readmeParity(
      english,
      { ...chinese, markdown: chinese.markdown.replace('(README.md)', '(README.en.md)') },
      '/repo'
    ),
    []
  )
  assert.deepEqual(
    readmeParity(
      {
        ...english,
        markdown: english.markdown.replace('(README.zh-CN.md)', '(README.zh-CN.md#快速开始)')
      },
      { ...chinese, markdown: chinese.markdown.replace('(README.md)', '(README.md#quick-start)') },
      '/repo'
    ),
    []
  )
})

test('bilingual parity rejects missing sections, images, links and navigation', () => {
  for (const [needle, replacement, expected] of [
    ['## 快速开始', '### 快速开始', /heading depth/],
    ['![截图](assets/screen.png)', '', /image targets/],
    ['[指南](docs/guide.md#installation)', '', /link targets/],
    ['[开始](#快速开始)', '', /link targets/]
  ]) {
    assert.match(
      readmeParity(
        english,
        { ...chinese, markdown: chinese.markdown.replace(needle, replacement) },
        '/repo'
      ).join('\n'),
      expected
    )
  }
})
