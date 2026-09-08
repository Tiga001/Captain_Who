/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { createHash } from 'node:crypto'
import { cp, lstat, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'

import {
  LICENSE_FILE_PATTERN,
  MAX_LEGAL_EVIDENCE_FILES_PER_PACKAGE,
  MAX_LEGAL_EVIDENCE_FILE_BYTES,
  NODE_METADATA_LICENSE_EVIDENCE_ALLOWLIST,
  PYTHON_METADATA_LICENSE_EVIDENCE_ALLOWLIST,
  artifactRuntimePythonLayout
} from './contract.mjs'
import { hashFile } from './filesystem.mjs'

export function normalizePythonDistributionName(name) {
  return name.toLowerCase().replaceAll('_', '-').replaceAll('.', '-')
}

function legalPackageDirectory(name, version) {
  const readable = `${name}@${version}`.replaceAll('@', '_').replaceAll('/', '__')
  const safe = readable.replaceAll(/[^A-Za-z0-9._+-]/g, '_').slice(0, 120)
  const suffix = createHash('sha256').update(`${name}@${version}`).digest('hex').slice(0, 12)
  return `${safe}-${suffix}`
}

function packageSource(document) {
  const repository =
    typeof document.repository === 'string' ? document.repository : document.repository?.url
  const source = repository ?? document.homepage
  return typeof source === 'string' && source.trim().length > 0 ? source.trim() : null
}

async function copyLegalEvidenceFile(source, destination, componentRoot) {
  const metadata = await lstat(source)
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error(`Legal evidence must be a regular non-symlink file: ${source}`)
  }
  if (metadata.size <= 0 || metadata.size > MAX_LEGAL_EVIDENCE_FILE_BYTES) {
    throw new Error(`Legal evidence has an invalid size: ${source}`)
  }
  await mkdir(dirname(destination), { recursive: true })
  await cp(source, destination, { force: false, errorOnExist: true })
  return Object.freeze({
    path: destination.slice(componentRoot.length + 1).replaceAll('\\', '/'),
    size: metadata.size,
    sha256: await hashFile(destination)
  })
}

async function collectLicenseFiles(root, { excludeNodeModules = false } = {}) {
  const pending = [{ directory: root, prefix: '' }]
  const found = []
  while (pending.length > 0) {
    const { directory, prefix } = pending.pop()
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      if (excludeNodeModules && entry.name === 'node_modules') continue
      const source = join(directory, entry.name)
      const relative = prefix ? `${prefix}/${entry.name}` : entry.name
      if (entry.isSymbolicLink()) {
        throw new Error(`Legal evidence scan encountered a symlink: ${source}`)
      }
      if (entry.isDirectory()) {
        pending.push({ directory: source, prefix: relative })
      } else if (entry.isFile() && LICENSE_FILE_PATTERN.test(entry.name)) {
        found.push({ source, relative })
        if (found.length > MAX_LEGAL_EVIDENCE_FILES_PER_PACKAGE) {
          throw new Error(`Package contains too many legal evidence files: ${root}`)
        }
      }
    }
  }
  return found.sort((left, right) => left.relative.localeCompare(right.relative, 'en'))
}

async function readNodePackageRoot(packageRoot) {
  const packageJson = join(packageRoot, 'package.json')
  const document = JSON.parse(await readFile(packageJson, 'utf8'))
  if (
    typeof document.name !== 'string' ||
    document.name.length === 0 ||
    typeof document.version !== 'string' ||
    document.version.length === 0
  ) {
    throw new Error(`Managed Node package has invalid identity metadata: ${packageJson}`)
  }
  return { packageRoot, packageJson, document }
}

async function collectNodePackageOccurrences(nodeModulesRoot) {
  const occurrences = []
  async function visitNodeModules(directory) {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      if (!entry.isDirectory() || entry.name.startsWith('.')) continue
      if (entry.name.startsWith('@')) {
        const scope = join(directory, entry.name)
        for (const child of await readdir(scope, { withFileTypes: true })) {
          if (child.isDirectory()) await visitPackage(join(scope, child.name))
        }
      } else {
        await visitPackage(join(directory, entry.name))
      }
    }
  }
  async function visitPackage(packageRoot) {
    const packageEntry = await readNodePackageRoot(packageRoot)
    occurrences.push(packageEntry)
    const nested = join(packageRoot, 'node_modules')
    try {
      const metadata = await lstat(nested)
      if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
        throw new Error(`Managed Node node_modules entry is not a regular directory: ${nested}`)
      }
      await visitNodeModules(nested)
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
    }
  }
  await visitNodeModules(nodeModulesRoot)
  return occurrences
}

async function buildNodeLegalInventory(manifest, staging) {
  const nodeModulesRoot = join(staging, ...manifest.node.packageRoot.split('/'))
  const occurrences = await collectNodePackageOccurrences(nodeModulesRoot)
  const grouped = new Map()
  for (const occurrence of occurrences) {
    const key = `${occurrence.document.name}@${occurrence.document.version}`
    const existing = grouped.get(key) ?? []
    existing.push(occurrence)
    grouped.set(key, existing)
  }
  const direct = new Set(
    manifest.node.dependencies.map(({ name, version }) => `${name}@${version}`)
  )
  const inventory = []
  for (const key of [...grouped.keys()].sort()) {
    const packageOccurrences = grouped.get(key)
    const representative = packageOccurrences[0]
    const { document, packageRoot, packageJson } = representative
    const source = packageSource(document)
    if (!source) throw new Error(`Node package ${key} does not declare a source or homepage`)
    const declaredLicense = typeof document.license === 'string' ? document.license : null
    const licenseFiles = await collectLicenseFiles(packageRoot, { excludeNodeModules: true })
    const exception = NODE_METADATA_LICENSE_EVIDENCE_ALLOWLIST[key]
    if (licenseFiles.length === 0) {
      if (!exception || exception.declaredLicense !== declaredLicense) {
        throw new Error(`Node package ${key} has no license file or exact reviewed exception`)
      }
    } else if (!declaredLicense) {
      throw new Error(`Node package ${key} has license files but no declared license expression`)
    }
    const destinationRoot = join(
      staging,
      'legal',
      'node-packages',
      legalPackageDirectory(document.name, document.version)
    )
    const evidence = [
      {
        kind: 'packageMetadata',
        ...(await copyLegalEvidenceFile(
          packageJson,
          join(destinationRoot, 'package.json'),
          staging
        ))
      }
    ]
    for (const candidate of licenseFiles) {
      evidence.push({
        kind: 'licenseFile',
        ...(await copyLegalEvidenceFile(
          candidate.source,
          join(destinationRoot, 'licenses', ...candidate.relative.split('/')),
          staging
        ))
      })
    }
    if (licenseFiles.length === 0) {
      for (const relative of exception.evidenceFiles) {
        if (relative === 'package.json') continue
        evidence.push({
          kind: 'reviewedMetadataEvidence',
          ...(await copyLegalEvidenceFile(
            join(packageRoot, ...relative.split('/')),
            join(destinationRoot, 'reviewed-evidence', ...relative.split('/')),
            staging
          ))
        })
      }
    }
    inventory.push({
      ecosystem: 'npm',
      name: document.name,
      version: document.version,
      direct: direct.has(key),
      licenseExpression: exception?.licenseExpression ?? declaredLicense,
      source,
      installPaths: packageOccurrences
        .map(({ packageRoot: path }) => path.slice(staging.length + 1).replaceAll('\\', '/'))
        .sort(),
      evidence,
      ...(exception ? { reviewedException: exception.review } : {})
    })
  }
  return inventory
}

function parsePythonMetadata(text, label) {
  const values = new Map()
  for (const line of text.split(/\r?\n/)) {
    const separator = line.indexOf(':')
    if (separator <= 0) continue
    const name = line.slice(0, separator).toLowerCase()
    const value = line.slice(separator + 1).trim()
    const existing = values.get(name) ?? []
    existing.push(value)
    values.set(name, existing)
  }
  const first = (name) => values.get(name)?.find((value) => value.length > 0) ?? null
  const name = first('name')
  const version = first('version')
  if (!name || !version) throw new Error(`Python distribution has invalid METADATA: ${label}`)
  const projectUrls = values.get('project-url') ?? []
  const preferredSource = ['source', 'source code', 'repository', 'homepage', 'home', 'code']
    .map((kind) =>
      projectUrls.find((entry) => entry.slice(0, entry.indexOf(',')).trim().toLowerCase() === kind)
    )
    .find(Boolean)
  const source =
    preferredSource?.slice(preferredSource.indexOf(',') + 1).trim() ?? first('home-page')
  return {
    name,
    version,
    licenseExpression: first('license-expression') ?? first('license'),
    source
  }
}

async function findPythonDistributionMetadata(runtimeRoot) {
  const pending = [runtimeRoot]
  const found = []
  while (pending.length > 0) {
    const directory = pending.pop()
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      if (entry.isSymbolicLink()) throw new Error(`Managed Python tree contains a symlink: ${path}`)
      if (!entry.isDirectory()) continue
      if (entry.name.endsWith('.dist-info')) {
        const metadata = join(path, 'METADATA')
        try {
          if ((await lstat(metadata)).isFile()) found.push({ directory: path, metadata })
        } catch (error) {
          if (error?.code !== 'ENOENT') throw error
        }
      } else {
        pending.push(path)
      }
    }
  }
  return found
}

async function buildPythonLegalInventory(manifest, staging) {
  const runtimeRoot = join(staging, ...manifest.python.runtimeHome.split('/'))
  const distributions = await findPythonDistributionMetadata(runtimeRoot)
  const direct = new Set(
    manifest.python.dependencies.map(
      ({ name, version }) => `${normalizePythonDistributionName(name)}@${version}`
    )
  )
  const inventory = []
  const seen = new Set()
  for (const distribution of distributions) {
    const metadataText = await readFile(distribution.metadata, 'utf8')
    const parsed = parsePythonMetadata(metadataText, distribution.metadata)
    const key = `${normalizePythonDistributionName(parsed.name)}@${parsed.version}`
    if (seen.has(key)) throw new Error(`Managed Python contains duplicate distribution ${key}`)
    seen.add(key)
    if (!parsed.source) throw new Error(`Python distribution ${key} does not declare a source`)
    const licenseFiles = await collectLicenseFiles(distribution.directory)
    const exception = PYTHON_METADATA_LICENSE_EVIDENCE_ALLOWLIST[key]
    if (exception && exception.declaredLicense !== parsed.licenseExpression) {
      throw new Error(`Python distribution ${key} no longer matches its reviewed license metadata`)
    }
    const licenseExpression = exception?.licenseExpression ?? parsed.licenseExpression
    if (licenseFiles.length === 0) {
      if (!exception) {
        throw new Error(
          `Python distribution ${key} has no license file or exact reviewed exception`
        )
      }
    } else if (!licenseExpression) {
      throw new Error(`Python distribution ${key} has no declared license expression`)
    }
    const destinationRoot = join(
      staging,
      'legal',
      'python-packages',
      legalPackageDirectory(parsed.name, parsed.version)
    )
    const evidence = [
      {
        kind: 'distributionMetadata',
        ...(await copyLegalEvidenceFile(
          distribution.metadata,
          join(destinationRoot, 'METADATA'),
          staging
        ))
      }
    ]
    for (const candidate of licenseFiles) {
      evidence.push({
        kind: 'licenseFile',
        ...(await copyLegalEvidenceFile(
          candidate.source,
          join(destinationRoot, 'licenses', ...candidate.relative.split('/')),
          staging
        ))
      })
    }
    inventory.push({
      ecosystem: 'pypi',
      name: parsed.name,
      version: parsed.version,
      direct: direct.has(key),
      licenseExpression,
      source: parsed.source,
      installPath: distribution.directory.slice(staging.length + 1).replaceAll('\\', '/'),
      evidence,
      ...(exception ? { reviewedException: exception.review } : {})
    })
  }
  inventory.sort((left, right) =>
    `${left.name}@${left.version}`.localeCompare(`${right.name}@${right.version}`, 'en')
  )
  for (const expected of direct) {
    if (!seen.has(expected))
      throw new Error(`Managed Python is missing direct dependency ${expected}`)
  }
  return inventory
}

export async function prepareArtifactRuntimeLegalEvidence(
  manifest,
  staging,
  platform = process.platform
) {
  await rm(join(staging, 'component-legal.json'), { force: true })
  await rm(join(staging, 'legal', 'node-packages'), { recursive: true, force: true })
  await rm(join(staging, 'legal', 'python-packages'), { recursive: true, force: true })
  await rm(join(staging, 'legal', 'python-runtime'), { recursive: true, force: true })

  const nodeRuntimeLicense = join(staging, ...manifest.node.licenseFile.target.split('/'))
  if (!(await lstat(nodeRuntimeLicense)).isFile()) {
    throw new Error('Managed Node runtime license evidence is missing')
  }
  const pythonRuntimeLicenseSource = join(
    staging,
    ...artifactRuntimePythonLayout(manifest, platform).licenseFile.split('/')
  )
  const pythonRuntimeLicense = await copyLegalEvidenceFile(
    pythonRuntimeLicenseSource,
    join(staging, 'legal', 'python-runtime', 'CPython-LICENSE.txt'),
    staging
  )
  const nodePackages = await buildNodeLegalInventory(manifest, staging)
  const pythonPackages = await buildPythonLegalInventory(manifest, staging)
  const legalManifest = {
    schemaVersion: 3,
    providerId: manifest.providerId,
    bundleVersion: manifest.bundleVersion,
    runtimes: [
      {
        name: 'Node.js',
        version: manifest.node.version,
        licenseExpression: 'MIT',
        source: 'https://github.com/nodejs/node',
        evidence: [{ kind: 'licenseFile', path: manifest.node.licenseFile.target }]
      },
      {
        name: 'python-build-standalone / CPython',
        version: `${manifest.python.version}+${manifest.python.release}`,
        licenseExpression: manifest.python.license,
        source: manifest.python.source,
        evidence: [
          { kind: 'licenseFile', ...pythonRuntimeLicense },
          {
            kind: 'pinnedManifestMetadata',
            path: 'runtime-manifest.json',
            note: 'python-build-standalone source and MPL-2.0 expression are pinned here'
          }
        ]
      }
    ],
    tools: [
      {
        name: 'ripgrep',
        version: manifest.tools.ripgrep.version,
        licenseExpression: manifest.tools.ripgrep.license,
        source: manifest.tools.ripgrep.source,
        evidence: manifest.tools.ripgrep.licenseFiles.map(({ target }) => ({
          kind: 'licenseFile',
          path: target
        }))
      }
    ],
    packages: { node: nodePackages, python: pythonPackages }
  }
  await writeFile(
    join(staging, 'component-legal.json'),
    `${JSON.stringify(legalManifest, null, 2)}\n`,
    { flag: 'wx', mode: 0o600 }
  )
  return legalManifest
}
