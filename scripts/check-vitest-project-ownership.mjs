/* eslint-disable @typescript-eslint/explicit-function-return-type -- The checker is runtime-validated JavaScript. */
import { globSync, statSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import { vitestCandidateTestGlobs, vitestProjectFileRules } from './vitest-project-rules.mjs'

const defaultRepositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')

function normalizeRepositoryPath(file) {
  return file.replaceAll('\\', '/')
}

function findRegularFiles(repositoryRoot, patterns) {
  const files = new Set()
  for (const file of globSync(patterns, { cwd: repositoryRoot })) {
    if (statSync(join(repositoryRoot, file)).isFile()) {
      files.add(normalizeRepositoryPath(file))
    }
  }
  return [...files].sort()
}

export function analyzeVitestProjectOwnership({
  repositoryRoot = defaultRepositoryRoot,
  candidatePatterns = vitestCandidateTestGlobs,
  projectRules = vitestProjectFileRules
} = {}) {
  const candidateFiles = findRegularFiles(repositoryRoot, candidatePatterns)
  const ownersByFile = new Map(candidateFiles.map((file) => [file, []]))
  const projectCounts = {}
  const unexpectedProjectFiles = []

  for (const [projectName, rule] of Object.entries(projectRules)) {
    const excludedFiles = new Set(findRegularFiles(repositoryRoot, rule.exclude ?? []))
    const projectFiles = findRegularFiles(repositoryRoot, rule.include).filter(
      (file) => !excludedFiles.has(file)
    )
    projectCounts[projectName] = projectFiles.length

    for (const file of projectFiles) {
      const owners = ownersByFile.get(file)
      if (!owners) {
        unexpectedProjectFiles.push({ file, project: projectName })
        continue
      }
      owners.push(projectName)
    }
  }

  const unownedFiles = []
  const multiplyOwnedFiles = []
  for (const [file, owners] of ownersByFile) {
    if (owners.length === 0) unownedFiles.push(file)
    if (owners.length > 1) multiplyOwnedFiles.push({ file, projects: owners })
  }

  return {
    candidateFiles,
    projectCounts,
    unownedFiles,
    multiplyOwnedFiles,
    unexpectedProjectFiles
  }
}

export function assertVitestProjectOwnership(options) {
  const analysis = analyzeVitestProjectOwnership(options)
  const failures = [
    ...analysis.unownedFiles.map((file) => `${file}: unowned`),
    ...analysis.multiplyOwnedFiles.map(
      ({ file, projects }) => `${file}: owned by multiple projects (${projects.join(', ')})`
    ),
    ...analysis.unexpectedProjectFiles.map(
      ({ file, project }) => `${file}: selected by ${project} but outside candidate test globs`
    )
  ]

  if (failures.length > 0) {
    throw new Error(`Vitest project ownership check failed:\n${failures.join('\n')}`)
  }

  return analysis
}

function formatSuccess(analysis) {
  const counts = Object.entries(analysis.projectCounts)
    .map(([project, count]) => `${project}=${count}`)
    .join(', ')
  return `Vitest project ownership OK: ${analysis.candidateFiles.length} files (${counts})`
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : null
if (invokedPath === import.meta.url) {
  try {
    console.log(formatSuccess(assertVitestProjectOwnership()))
  } catch (error) {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  }
}
