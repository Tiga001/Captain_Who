/* eslint-disable @typescript-eslint/explicit-function-return-type -- Node test helpers are runtime-validated JavaScript. */
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'

import {
  analyzeVitestProjectOwnership,
  assertVitestProjectOwnership
} from './check-vitest-project-ownership.mjs'

const candidatePatterns = ['src/**/*.{test,spec}.{ts,tsx}', 'packages/**/*.{test,spec}.{ts,tsx}']

async function withFixture(files, callback) {
  const repositoryRoot = await mkdtemp(join(tmpdir(), 'mycopilot-vitest-ownership-'))
  try {
    for (const file of files) {
      const absolutePath = join(repositoryRoot, file)
      await mkdir(dirname(absolutePath), { recursive: true })
      await writeFile(absolutePath, '// fixture\n', 'utf8')
    }
    await callback(repositoryRoot)
  } finally {
    await rm(repositoryRoot, { recursive: true, force: true })
  }
}

test('accepts exactly one owner per TS and TSX test file', async () => {
  await withFixture(
    [
      'src/domain/value.test.ts',
      'src/main/window.electron.test.ts',
      'src/ui/view.browser.test.tsx',
      'packages/protocol/src/parser.spec.ts'
    ],
    async (repositoryRoot) => {
      await mkdir(join(repositoryRoot, 'src/ui/__screenshots__/view.browser.test.tsx'), {
        recursive: true
      })
      const analysis = assertVitestProjectOwnership({
        repositoryRoot,
        candidatePatterns,
        projectRules: {
          unit: {
            include: ['src/**/*.test.ts', 'packages/**/*.spec.ts'],
            exclude: ['src/**/*.electron.test.ts']
          },
          browser: { include: ['src/**/*.browser.test.tsx'], exclude: [] },
          electron: { include: ['src/**/*.electron.test.ts'], exclude: [] }
        }
      })

      assert.equal(analysis.candidateFiles.length, 4)
      assert.deepEqual(analysis.projectCounts, { unit: 2, browser: 1, electron: 1 })
    }
  )
})

test('keeps the repository test inventory in its intended projects', () => {
  const analysis = assertVitestProjectOwnership()

  assert.deepEqual(Object.keys(analysis.projectCounts).sort(), [
    'automation-core-e2e',
    'browser',
    'electron-fixtures',
    'human-interaction-core-e2e',
    'managed-playwright-e2e',
    'unit'
  ])
  for (const [project, count] of Object.entries(analysis.projectCounts)) {
    assert.ok(count > 0, `${project} must select repository tests`)
  }
  assert.equal(
    Object.values(analysis.projectCounts).reduce((total, count) => total + count, 0),
    analysis.candidateFiles.length
  )
})

test('repository rules route growing inventories to their intended projects', async () => {
  await withFixture(
    [
      'src/main/core/newFeature.test.ts',
      'packages/protocol/src/newFeature.test.ts',
      'src/renderer/src/features/settings/__tests__/newFeature.test.tsx',
      'src/renderer/src/features/settings/__tests__/newFeature.browser.test.tsx',
      'src/renderer/src/features/chat/__tests__/newFeature.browser.test.tsx',
      'src/main/core/newFeature.electron.test.ts',
      'src/main/core/managedPlaywrightBridge.electron.test.ts',
      'src/main/core/humanInteractionRealCore.integration.test.ts',
      'src/main/core/automationHostRealCore.integration.test.ts'
    ],
    async (repositoryRoot) => {
      const analysis = assertVitestProjectOwnership({ repositoryRoot })
      assert.equal(analysis.candidateFiles.length, 9)
      assert.deepEqual(analysis.projectCounts, {
        unit: 3,
        browser: 2,
        'electron-fixtures': 1,
        'managed-playwright-e2e': 1,
        'human-interaction-core-e2e': 1,
        'automation-core-e2e': 1
      })
    }
  )
})

test('reports unowned test files', async () => {
  await withFixture(['src/domain/orphan.test.tsx'], async (repositoryRoot) => {
    assert.throws(
      () =>
        assertVitestProjectOwnership({
          repositoryRoot,
          candidatePatterns,
          projectRules: { unit: { include: ['src/**/*.test.ts'], exclude: [] } }
        }),
      /src\/domain\/orphan\.test\.tsx: unowned/
    )
  })
})

test('reports files selected by more than one project', async () => {
  await withFixture(['src/domain/duplicate.test.ts'], async (repositoryRoot) => {
    const options = {
      repositoryRoot,
      candidatePatterns,
      projectRules: {
        unit: { include: ['src/**/*.test.ts'], exclude: [] },
        duplicate: { include: ['src/domain/*.test.ts'], exclude: [] }
      }
    }

    const analysis = analyzeVitestProjectOwnership(options)
    assert.deepEqual(analysis.multiplyOwnedFiles, [
      {
        file: 'src/domain/duplicate.test.ts',
        projects: ['unit', 'duplicate']
      }
    ])
    assert.throws(
      () => assertVitestProjectOwnership(options),
      /src\/domain\/duplicate\.test\.ts: owned by multiple projects \(unit, duplicate\)/
    )
  })
})

test('reports project files outside the candidate test globs', async () => {
  await withFixture(['scripts/extra.test.ts'], async (repositoryRoot) => {
    assert.throws(
      () =>
        assertVitestProjectOwnership({
          repositoryRoot,
          candidatePatterns,
          projectRules: { unit: { include: ['scripts/**/*.test.ts'], exclude: [] } }
        }),
      /scripts\/extra\.test\.ts: selected by unit but outside candidate test globs/
    )
  })
})
