/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { createHash } from 'node:crypto'
import { createRequire, isBuiltin } from 'node:module'
import { lstat, mkdir, readFile, readdir, realpath, writeFile } from 'node:fs/promises'
import { dirname, isAbsolute, join, relative } from 'node:path'

import { readRegularFileNoFollow } from './filesystem.mjs'
import {
  BUILD_INPUT_RELATIVE_PATHS,
  BUILD_INPUT_SOURCE_PATHS,
  MAX_NODE_PACKAGE_BYTES,
  MAX_NODE_PACKAGE_EVIDENCE_BYTES,
  MAX_NODE_PACKAGE_FILES,
  NODE_PACKAGE_EVIDENCE_SOURCE,
  NODE_PACKAGE_EVIDENCE_TARGET,
  NODE_WORKSPACE_PACKAGE,
  REPOSITORY_ROOT,
  canonicalRelativePath,
  nonEmptyString
} from './contract.mjs'

// pnpm preserves CRLF shebangs on Windows but strips their CR on Unix. Match the frozen
// Unix package bytes without normalizing any other line or relaxing the content digest.
export function normalizeManagedNodeBytes(bytes, platform = process.platform) {
  if (platform !== 'win32' || bytes[0] !== 0x23 || bytes[1] !== 0x21) return bytes
  const newline = bytes.indexOf(0x0a)
  if (newline < 1 || bytes[newline - 1] !== 0x0d) return bytes
  return Buffer.concat([bytes.subarray(0, newline - 1), bytes.subarray(newline)])
}

function pathIsWithin(root, candidate) {
  const remainder = relative(root, candidate)
  return (
    remainder === '' ||
    (!isAbsolute(remainder) &&
      remainder !== '..' &&
      !remainder.startsWith('../') &&
      !remainder.startsWith('..\\'))
  )
}

async function canonicalDirectoryWithin(path, allowedRoot, label) {
  const metadata = await lstat(path)
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    throw new Error(`${label} must be a real, non-symlink directory: ${path}`)
  }
  const canonical = await realpath(path)
  if (!pathIsWithin(allowedRoot, canonical)) {
    throw new Error(`${label} escapes the approved package source boundary: ${path}`)
  }
  return canonical
}

async function findPackageRoot(name, fromDirectory, allowedPackageRoot) {
  const resolver = createRequire(join(fromDirectory, 'package.json'))
  let entry
  try {
    entry = resolver.resolve(`${name}/package.json`)
  } catch {
    try {
      entry = resolver.resolve(name)
    } catch (error) {
      const missing = new Error(
        `Cannot resolve locked Node dependency ${name} from ${fromDirectory}`,
        {
          cause: error
        }
      )
      missing.code = 'MANAGED_NODE_DEPENDENCY_NOT_FOUND'
      throw missing
    }
  }
  let current = dirname(entry)
  while (true) {
    const canonical = await canonicalDirectoryWithin(
      current,
      allowedPackageRoot,
      `Node dependency ${name}`
    )
    const packageJson = join(current, 'package.json')
    try {
      const { bytes } = await readRegularFileNoFollow(
        packageJson,
        `Node dependency ${name} package metadata`
      )
      const document = JSON.parse(bytes.toString('utf8'))
      if (document.name === name) return { root: canonical, document }
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
    }
    const parent = dirname(current)
    if (parent === current) break
    current = parent
  }
  throw new Error(`Cannot locate package root for ${name}`)
}

function unquotePnpmKey(value) {
  if (value.startsWith("'") && value.endsWith("'")) {
    return value.slice(1, -1).replaceAll("''", "'")
  }
  if (value.startsWith('"') && value.endsWith('"')) return JSON.parse(value)
  return value
}

export function parsePnpmLockPackageIntegrities(text) {
  if (typeof text !== 'string' || text.includes('\0')) {
    throw new Error('pnpm lockfile must be NUL-free UTF-8 text')
  }
  const integrities = new Map()
  let inPackages = false
  let packageKey = null
  for (const line of text.split(/\r?\n/)) {
    if (line === 'packages:') {
      inPackages = true
      continue
    }
    if (inPackages && /^\S/.test(line)) break
    if (!inPackages) continue
    const packageMatch = /^ {2}(\S.*):$/.exec(line)
    if (packageMatch) {
      packageKey = unquotePnpmKey(packageMatch[1])
      continue
    }
    const integrityMatch = /^ {4}resolution: \{[^}]*integrity: ([^, }]+)[^}]*\}$/.exec(line)
    if (packageKey && integrityMatch) {
      const integrity = integrityMatch[1]
      if (!/^sha(?:256|384|512)-[A-Za-z0-9+/]+={0,2}$/.test(integrity)) {
        throw new Error(`pnpm lock package ${packageKey} has an invalid integrity`)
      }
      integrities.set(packageKey, integrity)
    }
  }
  if (!inPackages || integrities.size === 0) {
    throw new Error('pnpm lockfile does not contain a package integrity table')
  }
  return integrities
}

function lockedIntegrityForPackage(integrities, name, version) {
  const identity = `${name}@${version}`
  const matches = new Set()
  for (const [key, integrity] of integrities) {
    if (key === identity || key.startsWith(`${identity}(`)) matches.add(integrity)
  }
  if (matches.size !== 1) {
    throw new Error(
      `pnpm lockfile must provide exactly one integrity for managed Node package ${identity}`
    )
  }
  return [...matches][0]
}

async function collectNodePackageFiles(packageRoot) {
  const pending = [{ directory: packageRoot, prefix: '' }]
  const files = []
  let totalBytes = 0
  while (pending.length > 0) {
    const { directory, prefix } = pending.pop()
    const entries = await readdir(directory, { withFileTypes: true })
    entries.sort((left, right) => left.name.localeCompare(right.name, 'en'))
    for (const entry of entries) {
      if (prefix === '' && entry.name === 'node_modules') continue
      const source = join(directory, entry.name)
      const logical = prefix ? `${prefix}/${entry.name}` : entry.name
      canonicalRelativePath(logical, 'managed Node package file path')
      const metadata = await lstat(source)
      if (metadata.isSymbolicLink()) {
        throw new Error(`Managed Node package contains a forbidden symlink: ${source}`)
      }
      if (metadata.isDirectory()) {
        pending.push({ directory: source, prefix: logical })
        continue
      }
      if (!metadata.isFile()) {
        throw new Error(`Managed Node package contains a non-regular entry: ${source}`)
      }
      if (files.length === MAX_NODE_PACKAGE_FILES) {
        throw new Error('Managed Node package graph exceeds the file-count limit')
      }
      totalBytes += metadata.size
      if (totalBytes > MAX_NODE_PACKAGE_BYTES) {
        throw new Error('Managed Node package graph exceeds the byte limit')
      }
      const { bytes: sourceBytes, metadata: opened } = await readRegularFileNoFollow(
        source,
        'Managed Node package content'
      )
      const bytes = normalizeManagedNodeBytes(sourceBytes)
      files.push({
        path: logical,
        size: bytes.length,
        sha256: createHash('sha256').update(bytes).digest('hex'),
        executable: (opened.mode & 0o111) !== 0
      })
    }
  }
  files.sort((left, right) => Buffer.from(left.path).compare(Buffer.from(right.path)))
  return files
}

async function collectManagedNodeGraph({
  sourcePackageDirectory,
  allowedPackageRoot,
  rootDependencies,
  lockIntegrities
}) {
  const packages = new Map()
  const rootsByPath = new Map()

  async function visit(name, fromDirectory) {
    const resolved = await findPackageRoot(name, fromDirectory, allowedPackageRoot)
    if (rootsByPath.has(resolved.root)) return rootsByPath.get(resolved.root)
    const version = nonEmptyString(resolved.document.version, `${name} package version`)
    const id = `${name}@${version}`
    rootsByPath.set(resolved.root, id)
    const files = await collectNodePackageFiles(resolved.root)
    const required = Object.keys(resolved.document.dependencies ?? {})
    const optional = Object.keys(resolved.document.optionalDependencies ?? {})
    const dependencies = []
    const omittedOptionalDependencies = []
    const builtins = []
    for (const dependency of [...new Set([...required, ...optional])].sort()) {
      if (isBuiltin(dependency)) {
        builtins.push(dependency)
        continue
      }
      try {
        dependencies.push({
          name: dependency,
          package: await visit(dependency, resolved.root),
          optional: optional.includes(dependency) && !required.includes(dependency)
        })
      } catch (error) {
        if (
          optional.includes(dependency) &&
          !required.includes(dependency) &&
          error?.code === 'MANAGED_NODE_DEPENDENCY_NOT_FOUND'
        ) {
          omittedOptionalDependencies.push(dependency)
          continue
        }
        throw error
      }
    }
    const record = {
      id,
      name,
      version,
      integrity: lockedIntegrityForPackage(lockIntegrities, name, version),
      contentRevision: `sha256:${createHash('sha256').update(JSON.stringify(files)).digest('hex')}`,
      dependencies,
      omittedOptionalDependencies,
      builtins,
      files
    }
    const existing = packages.get(id)
    if (existing && JSON.stringify(existing) !== JSON.stringify(record)) {
      throw new Error(`Managed Node package ${id} has inconsistent content or dependency graphs`)
    }
    packages.set(id, record)
    return id
  }

  const roots = []
  for (const dependency of rootDependencies) {
    const packageId = await visit(dependency.name, sourcePackageDirectory)
    if (packageId !== `${dependency.name}@${dependency.version}`) {
      throw new Error(
        `Locked Node dependency ${dependency.name} expected ${dependency.version}, got ${packageId}`
      )
    }
    roots.push({ name: dependency.name, package: packageId })
  }
  const sortedPackages = [...packages.values()].sort((left, right) =>
    Buffer.from(left.id).compare(Buffer.from(right.id))
  )
  return {
    roots,
    packages: sortedPackages,
    graphRevision: `sha256:${createHash('sha256')
      .update(JSON.stringify({ roots, packages: sortedPackages }))
      .digest('hex')}`
  }
}

export async function createManagedNodeDependencyEvidence({
  sourcePackageDirectory = NODE_WORKSPACE_PACKAGE,
  packageManifestPath = join(NODE_WORKSPACE_PACKAGE, 'package.json'),
  lockfilePath = BUILD_INPUT_SOURCE_PATHS.pnpmLockfile,
  rootDependencies,
  allowedPackageRoot
} = {}) {
  const packageManifestBytes = await readFile(packageManifestPath)
  const packageManifest = JSON.parse(packageManifestBytes.toString('utf8'))
  const dependencies =
    rootDependencies ??
    Object.entries(packageManifest.dependencies ?? {}).map(([name, version]) => ({ name, version }))
  const lockfileBytes = await readFile(lockfilePath)
  const boundary = await realpath(
    allowedPackageRoot ?? join(REPOSITORY_ROOT, 'node_modules', '.pnpm')
  )
  const graph = await inspectManagedNodeGraph({
    sourcePackageDirectory,
    allowedPackageRoot: boundary,
    rootDependencies: dependencies,
    lockfilePath
  })
  return {
    schemaVersion: 1,
    lockfile: {
      path: BUILD_INPUT_RELATIVE_PATHS.pnpmLockfile,
      sha256: createHash('sha256').update(lockfileBytes).digest('hex')
    },
    importer: {
      path: BUILD_INPUT_RELATIVE_PATHS.nodePackageManifest,
      sha256: createHash('sha256').update(packageManifestBytes).digest('hex')
    },
    ...graph
  }
}

async function inspectManagedNodeGraph({
  sourcePackageDirectory,
  allowedPackageRoot,
  rootDependencies,
  lockfilePath
}) {
  const lockfileBytes = await readFile(lockfilePath)
  const lockIntegrities = parsePnpmLockPackageIntegrities(lockfileBytes.toString('utf8'))
  return collectManagedNodeGraph({
    sourcePackageDirectory,
    allowedPackageRoot,
    rootDependencies,
    lockIntegrities
  })
}

async function loadManagedNodeDependencyEvidence(manifest) {
  const bytes = await readFile(NODE_PACKAGE_EVIDENCE_SOURCE)
  if (bytes.length === 0 || bytes.length > MAX_NODE_PACKAGE_EVIDENCE_BYTES || bytes.includes(0)) {
    throw new Error('Managed Node dependency evidence is invalid or oversized')
  }
  const evidence = JSON.parse(bytes.toString('utf8'))
  if (
    evidence.schemaVersion !== 1 ||
    evidence.lockfile?.path !== BUILD_INPUT_RELATIVE_PATHS.pnpmLockfile ||
    evidence.lockfile?.sha256 !== manifest.buildInputs.pnpmLockfile.sha256 ||
    evidence.importer?.path !== BUILD_INPUT_RELATIVE_PATHS.nodePackageManifest ||
    evidence.importer?.sha256 !== manifest.buildInputs.nodePackageManifest.sha256 ||
    !Array.isArray(evidence.roots) ||
    !Array.isArray(evidence.packages) ||
    !/^sha256:[a-f0-9]{64}$/.test(evidence.graphRevision)
  ) {
    throw new Error('Managed Node dependency evidence does not match the frozen build inputs')
  }
  return evidence
}

async function copyVerifiedPackageFiles(sourceRoot, destination, packageEvidence) {
  await mkdir(destination, { recursive: true })
  for (const file of packageEvidence.files) {
    const logical = canonicalRelativePath(file.path, `${packageEvidence.id} evidence path`)
    const source = join(sourceRoot, ...logical.split('/'))
    const target = join(destination, ...logical.split('/'))
    const { bytes: sourceBytes, metadata } = await readRegularFileNoFollow(
      source,
      `${packageEvidence.id} verified content`
    )
    const bytes = normalizeManagedNodeBytes(sourceBytes)
    const digest = createHash('sha256').update(bytes).digest('hex')
    if (
      bytes.length !== file.size ||
      digest !== file.sha256 ||
      (process.platform !== 'win32' && ((metadata.mode & 0o111) !== 0) !== file.executable)
    ) {
      throw new Error(`Managed Node package content changed after verification: ${source}`)
    }
    await mkdir(dirname(target), { recursive: true })
    await writeFile(target, bytes, {
      flag: 'wx',
      mode: file.executable ? 0o755 : 0o644
    })
  }
}

async function copyPackageDependency(
  name,
  fromDirectory,
  destination,
  allowedPackageRoot,
  evidenceById,
  ancestors = new Set()
) {
  const resolved = await findPackageRoot(name, fromDirectory, allowedPackageRoot)
  if (ancestors.has(resolved.root)) return
  const id = `${resolved.document.name}@${resolved.document.version}`
  const packageEvidence = evidenceById.get(id)
  if (!packageEvidence || packageEvidence.name !== name) {
    throw new Error(`Managed Node dependency ${id} is absent from frozen package evidence`)
  }
  const nextAncestors = new Set(ancestors).add(resolved.root)
  await copyVerifiedPackageFiles(resolved.root, destination, packageEvidence)
  for (const dependency of packageEvidence.dependencies) {
    const childDestination = join(destination, 'node_modules', ...dependency.name.split('/'))
    await copyPackageDependency(
      dependency.name,
      resolved.root,
      childDestination,
      allowedPackageRoot,
      evidenceById,
      nextAncestors
    )
  }
}

// Windows stat does not preserve POSIX execute bits. Restore only that metadata from the
// frozen evidence; file paths, sizes, hashes, and dependency edges still compare exactly.
export function normalizeManagedNodeGraph(graph, evidence, platform = process.platform) {
  if (platform !== 'win32') return graph
  const expectedPackages = new Map(evidence.packages.map((entry) => [entry.id, entry]))
  const packages = graph.packages.map((entry) => {
    const expectedFiles = new Map(
      (expectedPackages.get(entry.id)?.files ?? []).map((file) => [file.path, file])
    )
    const files = entry.files.map((file) => ({
      ...file,
      executable: expectedFiles.get(file.path)?.executable ?? file.executable
    }))
    return {
      ...entry,
      contentRevision: `sha256:${createHash('sha256').update(JSON.stringify(files)).digest('hex')}`,
      files
    }
  })
  return {
    ...graph,
    packages,
    graphRevision: `sha256:${createHash('sha256')
      .update(JSON.stringify({ roots: graph.roots, packages }))
      .digest('hex')}`
  }
}

export async function prepareManagedNodeDependencies(manifest, staging, options = {}) {
  const expected = options.evidence ?? (await loadManagedNodeDependencyEvidence(manifest))
  const sourcePackageDirectory = options.sourcePackageDirectory ?? NODE_WORKSPACE_PACKAGE
  const allowedPackageRoot = await realpath(
    options.allowedPackageRoot ?? join(REPOSITORY_ROOT, 'node_modules', '.pnpm')
  )
  const lockfilePath = options.lockfilePath ?? BUILD_INPUT_SOURCE_PATHS.pnpmLockfile
  const actualGraph = await inspectManagedNodeGraph({
    sourcePackageDirectory,
    lockfilePath,
    rootDependencies: manifest.node.dependencies,
    allowedPackageRoot
  })
  const expectedGraph = {
    roots: expected.roots,
    packages: expected.packages,
    graphRevision: expected.graphRevision
  }
  if (
    JSON.stringify(normalizeManagedNodeGraph(actualGraph, expected)) !==
    JSON.stringify(expectedGraph)
  ) {
    throw new Error(
      'Managed Node dependency graph or package bytes do not match the frozen supply-chain evidence'
    )
  }
  const root = join(staging, ...manifest.node.packageRoot.split('/'))
  await mkdir(root, { recursive: true })
  await writeFile(join(root, 'package.json'), '{"private":true,"type":"module"}\n', {
    flag: 'wx',
    mode: 0o600
  })
  const evidenceById = new Map(expected.packages.map((entry) => [entry.id, entry]))
  for (const dependency of manifest.node.dependencies) {
    const destination = join(root, ...dependency.name.split('/'))
    await copyPackageDependency(
      dependency.name,
      sourcePackageDirectory,
      destination,
      allowedPackageRoot,
      evidenceById
    )
    const document = JSON.parse(await readFile(join(destination, 'package.json'), 'utf8'))
    if (document.name !== dependency.name || document.version !== dependency.version) {
      throw new Error(
        `Locked Node dependency ${dependency.name} expected ${dependency.version}, got ${document.version}`
      )
    }
  }
  const evidenceTarget = join(staging, ...NODE_PACKAGE_EVIDENCE_TARGET.split('/'))
  await mkdir(dirname(evidenceTarget), { recursive: true })
  await writeFile(evidenceTarget, `${JSON.stringify(expected, null, 2)}\n`, {
    flag: 'wx',
    mode: 0o644
  })
}

export async function verifyPreparedManagedNodeDependencies(manifest, staging) {
  const expected = await loadManagedNodeDependencyEvidence(manifest)
  const packageRoot = join(staging, ...manifest.node.packageRoot.split('/'))
  const allowedPackageRoot = await realpath(packageRoot)
  const actualGraph = await inspectManagedNodeGraph({
    sourcePackageDirectory: packageRoot,
    allowedPackageRoot,
    rootDependencies: manifest.node.dependencies,
    lockfilePath: BUILD_INPUT_SOURCE_PATHS.pnpmLockfile
  })
  const expectedGraph = {
    roots: expected.roots,
    packages: expected.packages,
    graphRevision: expected.graphRevision
  }
  if (
    JSON.stringify(normalizeManagedNodeGraph(actualGraph, expected)) !==
    JSON.stringify(expectedGraph)
  ) {
    throw new Error(
      'Offline Artifact Runtime Node dependencies do not match the frozen supply-chain evidence'
    )
  }
  const publishedEvidence = await readFile(
    join(staging, ...NODE_PACKAGE_EVIDENCE_TARGET.split('/'))
  )
  if (publishedEvidence.toString('utf8') !== `${JSON.stringify(expected, null, 2)}\n`) {
    throw new Error('Offline Artifact Runtime Node package evidence is missing or altered')
  }
}
