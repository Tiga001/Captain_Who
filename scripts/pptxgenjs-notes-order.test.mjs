/* eslint-disable @typescript-eslint/explicit-function-return-type -- Node's test runner infers helper contracts. */

import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import test from 'node:test'

const repositoryRoot = resolve(import.meta.dirname, '..')
const runtimePackageJson = join(repositoryRoot, 'packages', 'artifact-runtime-node', 'package.json')
const runtimeRequire = createRequire(runtimePackageJson)
const cjsEntry = runtimeRequire.resolve('pptxgenjs')
const dependencyRequire = createRequire(cjsEntry)
const JSZip = dependencyRequire('jszip')
const notesSentinels = ['managed-speaker-note-one', 'managed-speaker-note-two']

async function loadConstructors() {
  const CommonJsPptxGenJS = runtimeRequire('pptxgenjs')
  const esmEntry = join(dirname(cjsEntry), 'pptxgen.es.js')
  const { default: EsModulePptxGenJS } = await import(pathToFileURL(esmEntry).href)
  return [
    ['CommonJS', CommonJsPptxGenJS],
    ['ES module', EsModulePptxGenJS]
  ]
}

async function buildDeck(PptxGenJS, { slideCount, notes = [] }) {
  const presentation = new PptxGenJS()
  presentation.layout = 'LAYOUT_WIDE'
  for (let index = 0; index < slideCount; index += 1) {
    const slide = presentation.addSlide()
    slide.addText(`Managed slide ${index + 1}`, { x: 1, y: 1, w: 4, h: 1 })
    if (notes[index] !== undefined) slide.addNotes(notes[index])
  }
  return Buffer.from(await presentation.write({ outputType: 'nodebuffer' }))
}

async function archiveText(zip, path) {
  const entry = zip.file(path)
  assert.ok(entry, `generated presentation must contain ${path}`)
  return entry.async('string')
}

function assertPresentationChildOrder(presentationXml, buildKind) {
  const orderedChildren = [
    'sldMasterIdLst',
    'notesMasterIdLst',
    'sldIdLst',
    'sldSz',
    'notesSz',
    'defaultTextStyle'
  ]
  let previousOffset = -1
  for (const child of orderedChildren) {
    const offset = presentationXml.indexOf(`<p:${child}`)
    assert.notEqual(offset, -1, `${buildKind} output must contain p:${child}`)
    assert.ok(
      offset > previousOffset,
      `${buildKind} output must emit ${orderedChildren.join(' < ')}`
    )
    previousOffset = offset
  }
}

async function assertPresentationPackage(binary, buildKind, expectedNotes = []) {
  const zip = await JSZip.loadAsync(binary)
  const presentationXml = await archiveText(zip, 'ppt/presentation.xml')
  assertPresentationChildOrder(presentationXml, buildKind)

  const presentationRelationships = await archiveText(zip, 'ppt/_rels/presentation.xml.rels')
  assert.match(
    presentationRelationships,
    /Target="notesMasters\/notesMaster1\.xml"/,
    `${buildKind} output must retain the presentation-to-notes-master relationship`
  )
  await archiveText(zip, 'ppt/notesMasters/notesMaster1.xml')
  await archiveText(zip, 'ppt/notesMasters/_rels/notesMaster1.xml.rels')

  for (const [index, sentinel] of expectedNotes.entries()) {
    const slideNumber = index + 1
    const notesXml = await archiveText(zip, `ppt/notesSlides/notesSlide${slideNumber}.xml`)
    assert.match(
      notesXml,
      new RegExp(`<a:t>${sentinel}</a:t>`),
      `${buildKind} output must retain speaker notes for slide ${slideNumber}`
    )
    const notesRelationships = await archiveText(
      zip,
      `ppt/notesSlides/_rels/notesSlide${slideNumber}.xml.rels`
    )
    assert.match(notesRelationships, /relationships\/notesMaster/)
    assert.match(notesRelationships, /relationships\/slide/)
  }
}

function run(executable, args, { cwd, env } = {}) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(executable, args, {
      cwd,
      env: { ...process.env, ...env },
      shell: false,
      stdio: ['ignore', 'pipe', 'pipe']
    })
    const stdout = []
    const stderr = []
    child.stdout.on('data', (chunk) => stdout.push(chunk))
    child.stderr.on('data', (chunk) => stderr.push(chunk))
    child.once('error', rejectPromise)
    child.once('close', (code) => {
      resolvePromise({
        code,
        stdout: Buffer.concat(stdout).toString('utf8'),
        stderr: Buffer.concat(stderr).toString('utf8')
      })
    })
  })
}

test('managed PptxGenJS emits schema-ordered notes metadata and preserves speaker notes', async () => {
  const cases = [
    { label: 'one slide without addNotes', slideCount: 1 },
    { label: 'three slides without addNotes', slideCount: 3 },
    { label: 'two slides with speaker notes', slideCount: 2, notes: notesSentinels }
  ]
  for (const [moduleKind, PptxGenJS] of await loadConstructors()) {
    for (const deckCase of cases) {
      const buildKind = `${moduleKind}, ${deckCase.label}`
      await assertPresentationPackage(
        await buildDeck(PptxGenJS, deckCase),
        buildKind,
        deckCase.notes
      )
    }
  }
})

test(
  'managed PptxGenJS notes deck passes the pinned OfficeCLI strict schema gate',
  { skip: !process.env.MYCOPILOT_OFFICECLI_PATH },
  async () => {
    const directory = await mkdtemp(join(tmpdir(), 'mycopilot-pptxgenjs-notes-'))
    try {
      const [[, PptxGenJS]] = await loadConstructors()
      const output = join(directory, 'notes-order.pptx')
      await writeFile(output, await buildDeck(PptxGenJS, { slideCount: 2, notes: notesSentinels }))
      const result = await run(
        process.env.MYCOPILOT_OFFICECLI_PATH,
        ['validate', output, '--json'],
        {
          cwd: directory,
          env: {
            HOME: directory,
            TMPDIR: directory,
            OFFICECLI_SKIP_UPDATE: '1',
            OFFICECLI_NO_AUTO_RESIDENT: '1'
          }
        }
      )
      assert.equal(
        result.code,
        0,
        `OfficeCLI strict validation failed\nstdout: ${result.stdout}\nstderr: ${result.stderr}`
      )
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  }
)
