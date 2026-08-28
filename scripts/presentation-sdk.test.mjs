/* eslint-disable @typescript-eslint/explicit-function-return-type -- These Node tests exercise JavaScript runtime contracts. */

import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { cp, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { pathToFileURL } from 'node:url'

const sdkUrl = pathToFileURL(
  new URL('../resources/artifact-runtime/presentation-sdk.mjs', import.meta.url).pathname
)
const bootstrapPath = new URL('../resources/artifact-runtime/node-bootstrap.mjs', import.meta.url)
  .pathname
const loaderPath = new URL('../resources/artifact-runtime/node-loader.mjs', import.meta.url)
  .pathname
const sdkPath = new URL('../resources/artifact-runtime/presentation-sdk.mjs', import.meta.url)
  .pathname

async function loadFreshSdk() {
  const url = new URL(sdkUrl)
  url.searchParams.set('test', `${Date.now()}-${Math.random()}`)
  return import(url.href)
}

async function withPlan(testBody) {
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-presentation-sdk-test-'))
  const planPath = join(directory, 'plan.json')
  const previous = process.env.MYCOPILOT_PRESENTATION_EDIT_PLAN
  process.env.MYCOPILOT_PRESENTATION_EDIT_PLAN = planPath
  try {
    const sdk = await loadFreshSdk()
    await testBody(sdk)
    return JSON.parse(await readFile(planPath, 'utf8'))
  } finally {
    if (previous === undefined) delete process.env.MYCOPILOT_PRESENTATION_EDIT_PLAN
    else process.env.MYCOPILOT_PRESENTATION_EDIT_PLAN = previous
    await rm(directory, { recursive: true, force: true })
  }
}

async function runManagedEditor(source) {
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-presentation-editor-sandbox-'))
  const componentRoot = join(directory, 'component')
  const runtimeRoot = join(componentRoot, 'runtime')
  const moduleRoot = join(componentRoot, 'dependencies', 'node', 'node_modules')
  const planRoot = join(directory, 'plan')
  const scriptPath = join(directory, 'editor.mjs')
  await mkdir(runtimeRoot, { recursive: true })
  await mkdir(moduleRoot, { recursive: true })
  await mkdir(planRoot, { recursive: true })
  await cp(bootstrapPath, join(runtimeRoot, 'node-bootstrap.mjs'))
  await cp(loaderPath, join(runtimeRoot, 'node-loader.mjs'))
  await cp(sdkPath, join(runtimeRoot, 'presentation-sdk.mjs'))
  await writeFile(scriptPath, source)

  // macOS exposes the temporary root through `/var` -> `/private/var`. Node's permission model
  // evaluates the canonical path, so the Host must pass the same canonical identity to its
  // allowlists, bootstrap, script argv, and plan environment.
  const canonicalDirectory = await realpath(directory)
  const canonicalComponentRoot = await realpath(componentRoot)
  const canonicalRuntimeRoot = await realpath(runtimeRoot)
  const canonicalModuleRoot = await realpath(moduleRoot)
  const canonicalPlanRoot = await realpath(planRoot)
  const canonicalScriptPath = await realpath(scriptPath)
  const canonicalPlanPath = join(canonicalPlanRoot, 'edit-plan.json')

  const args = [
    '--import',
    join(canonicalRuntimeRoot, 'node-bootstrap.mjs'),
    '--permission',
    `--allow-fs-read=${canonicalComponentRoot}`,
    `--allow-fs-read=${canonicalScriptPath}`,
    `--allow-fs-write=${canonicalPlanRoot}`,
    '--disallow-code-generation-from-strings',
    canonicalScriptPath
  ]
  const result = await new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(process.execPath, args, {
      cwd: canonicalDirectory,
      env: {
        LANG: 'C.UTF-8',
        MYCOPILOT_ARTIFACT_NODE_MODULES: canonicalModuleRoot,
        MYCOPILOT_PRESENTATION_EDIT_MODE: 'v1',
        MYCOPILOT_PRESENTATION_EDITOR_ENTRY: canonicalScriptPath,
        MYCOPILOT_PRESENTATION_EDIT_PLAN: canonicalPlanPath
      },
      shell: false,
      stdio: ['ignore', 'pipe', 'pipe']
    })
    const stdout = []
    const stderr = []
    child.stdout.on('data', (chunk) => stdout.push(chunk))
    child.stderr.on('data', (chunk) => stderr.push(chunk))
    child.once('error', rejectPromise)
    child.once('close', (code, signal) => {
      resolvePromise({
        code,
        signal,
        stdout: Buffer.concat(stdout).toString('utf8'),
        stderr: Buffer.concat(stderr).toString('utf8')
      })
    })
  })
  let plan = null
  try {
    plan = JSON.parse(await readFile(canonicalPlanPath, 'utf8'))
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error
  }
  await rm(directory, { recursive: true, force: true })
  return { ...result, plan }
}

test('fixed presentation editor SDK accepts every materialized template operation shape', async () => {
  const plan = await withPlan(async ({ editPresentation, input, output }) => {
    await editPresentation({
      source: input('source.pptx'),
      destination: output('outputs/source-edited.pptx'),
      mode: 'saveAs',
      edit(deck) {
        deck.replaceText({
          target: '/slide[1]/shape[@id=42]',
          find: 'Old title',
          replace: 'Updated title'
        })
        deck.replaceImage({
          target: '/slide[2]/picture[@id=17]',
          source: input('media/hero.png')
        })
        deck.updateTableCell({
          target: '/slide[3]/table[@id=9]/row[2]/cell[3]',
          text: '42%'
        })
        deck.updateChart({
          target: '/slide[4]/chart[@id=11]',
          properties: {
            categories: ['Q1', 'Q2'],
            series: [{ name: 'Revenue', values: [12, 18] }]
          }
        })
        deck.set({
          target: '/slide[5]/shape[@id=23]',
          properties: { x: '1in', y: '1.25in' }
        })
        deck.move({
          target: '/slide[5]/shape[@id=23]',
          position: { type: 'after', target: '/slide[5]/shape[@id=24]' }
        })
        deck.remove({ target: '/slide[8]/shape[@id=31]' })
      }
    })
  })

  assert.equal(plan.schemaVersion, 1)
  assert.deepEqual(plan.source, { type: 'input', mountPath: 'source.pptx' })
  assert.deepEqual(plan.destination, { type: 'output', path: 'outputs/source-edited.pptx' })
  assert.equal(plan.mode, 'saveAs')
  assert.equal(plan.operations.length, 7)
  assert.deepEqual(plan.operations[2], {
    type: 'set',
    target: '/slide[3]/table[@id=9]/row[2]/cell[3]',
    properties: { text: '42%' },
    replacement: null,
    force: false
  })
  assert.equal(plan.operations[3].properties.categories, 'Q1,Q2')
  assert.equal(plan.operations[3].properties.series1, 'Revenue:12,18')
  assert.equal(
    Object.hasOwn(plan.operations[3].properties, 'series'),
    false,
    'OfficeCLI accepts series1..seriesN properties, not one JSON `series` property'
  )
  assert.deepEqual(plan.operations[5].position, {
    type: 'after',
    target: '/slide[5]/shape[@id=24]'
  })
})

test('fixed presentation editor SDK fails before writing a plan for empty edits', async () => {
  await assert.rejects(
    withPlan(async ({ editPresentation, input, output }) => {
      await editPresentation({
        source: input('source.pptx'),
        destination: output('outputs/source-edited.pptx'),
        edit() {
          // Intentionally empty to verify that the SDK rejects a no-op plan.
        }
      })
    }),
    /contains no operations/
  )
})

test('fixed presentation editor SDK cannot submit more than one plan', async () => {
  await assert.rejects(
    withPlan(async ({ editPresentation, input, output }) => {
      const request = {
        source: input('source.pptx'),
        destination: output('outputs/source-edited.pptx'),
        edit(deck) {
          deck.remove({ target: '/slide[1]/shape[@id=1]' })
        }
      }
      await editPresentation(request)
      await editPresentation(request)
    }),
    /only once/
  )
})

test('fixed presentation editor SDK rejects chart values that cannot be compiled losslessly', async () => {
  for (const properties of [
    { categories: ['Q1,adjusted'], series: [{ name: 'Revenue', values: [12] }] },
    { categories: ['Q1'], series: [{ name: 'Revenue:net', values: [12] }] },
    { categories: ['Q1', 'Q2'], series: [{ name: 'Revenue', values: [12] }] },
    { categories: ['Q1'], series: [{ name: 'Revenue', values: [Number.NaN] }] }
  ]) {
    await assert.rejects(
      withPlan(async ({ editPresentation, input, output }) => {
        await editPresentation({
          source: input('source.pptx'),
          destination: output('outputs/source-edited.pptx'),
          edit(deck) {
            deck.updateChart({ target: '/slide[1]/chart[@id=7]', properties })
          }
        })
      }),
      /cannot|must match|finite/
    )
  }
})

test('fixed presentation editor SDK uses zero-based integer z-order indices', async () => {
  const plan = await withPlan(async ({ editPresentation, input, output }) => {
    await editPresentation({
      source: input('source.pptx'),
      destination: output('outputs/source-edited.pptx'),
      edit(deck) {
        deck.move({
          target: '/slide[1]/shape[@id=7]',
          position: { type: 'index', index: 0 }
        })
      }
    })
  })
  assert.deepEqual(plan.operations[0].position, { type: 'index', index: 0 })

  for (const index of [-1, 0.5, '0']) {
    await assert.rejects(
      withPlan(async ({ editPresentation, input, output }) => {
        await editPresentation({
          source: input('source.pptx'),
          destination: output('outputs/source-edited.pptx'),
          edit(deck) {
            deck.move({
              target: '/slide[1]/shape[@id=7]',
              position: { type: 'index', index }
            })
          }
        })
      }),
      /non-negative integer/
    )
  }
})

test('fixed presentation editor SDK emits one terminal whole-slide operation', async () => {
  for (const apply of [
    (deck) =>
      deck.addSlide({
        layout: 'LAYOUT_WIDE',
        title: 'Appendix',
        body: 'Supporting detail',
        backgroundColor: 'F8FAFC'
      }),
    (deck) => deck.removeSlide({ slideNumber: 8 }),
    (deck) => deck.moveSlide({ slideNumber: 7, newIndex: 3 })
  ]) {
    const plan = await withPlan(async ({ editPresentation, input, output }) => {
      await editPresentation({
        source: input('source.pptx'),
        destination: output('outputs/source-edited.pptx'),
        edit(deck) {
          deck.replaceText({
            target: '/slide[1]/shape[@id=7]',
            find: 'old',
            replace: 'new'
          })
          apply(deck)
        }
      })
    })
    assert.equal(plan.operations.length, 2)
    const structural = plan.operations[1]
    assert.ok(
      structural.elementType === 'slide' || /^\/slide\[[1-9][0-9]*\]$/.test(structural.target)
    )
  }

  const moved = await withPlan(async ({ editPresentation, input, output }) => {
    await editPresentation({
      source: input('source.pptx'),
      destination: output('outputs/source-edited.pptx'),
      edit(deck) {
        deck.moveSlide({ slideNumber: 7, newIndex: 3 })
      }
    })
  })
  assert.deepEqual(moved.operations[0].position, { type: 'index', index: 2 })
})

test('fixed presentation editor SDK rejects whole-slide target drift before writing a plan', async () => {
  for (const edit of [
    (deck) => {
      deck.removeSlide({ slideNumber: 8 })
      deck.replaceText({ target: '/slide[1]/shape[@id=7]', find: 'old', replace: 'new' })
    },
    (deck) => {
      deck.removeSlide({ slideNumber: 8 })
      deck.moveSlide({ slideNumber: 7, newIndex: 3 })
    }
  ]) {
    await assert.rejects(
      withPlan(async ({ editPresentation, input, output }) => {
        await editPresentation({
          source: input('source.pptx'),
          destination: output('outputs/source-edited.pptx'),
          edit
        })
      }),
      /at most one whole-slide operation and it must be last/
    )
  }
})

test('Host editor sandbox runs the fixed SDK but rejects general Node capabilities', async () => {
  const valid = await runManagedEditor(`
    import { editPresentation, input, output } from '@mycopilot/presentation-sdk'
    await editPresentation({
      source: input('source.pptx'),
      destination: output('outputs/source-edited.pptx'),
      edit(deck) { deck.replaceText({ target: '/slide[1]/shape[@id=7]', find: 'old', replace: 'new' }) }
    })
  `)
  assert.equal(valid.code, 0, valid.stderr)
  assert.equal(valid.plan.operations.length, 1)

  const forbidden = [
    `import 'node:fs'`,
    `await import('node:child_process')`,
    `await import('node:worker_threads')`,
    `module.createRequire(import.meta.url)('node:http')`,
    `process.getBuiltinModule('node:fs')`,
    `process.kill(process.ppid, 0)`,
    `process._kill(process.ppid, 0)`,
    `await fetch('https://example.com')`,
    `new WebSocket('ws://127.0.0.1:9')`,
    `Function('return 1')()`
  ]
  for (const source of forbidden) {
    const denied = await runManagedEditor(source)
    assert.notEqual(denied.code, 0, `sandbox unexpectedly ran: ${source}`)
    assert.equal(denied.plan, null, `sandbox emitted a plan after forbidden code: ${source}`)
  }
})
