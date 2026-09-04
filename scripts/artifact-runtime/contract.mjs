/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { readFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { verifyPinnedLocalFile } from './filesystem.mjs'

export const ARTIFACT_RUNTIME_MAX_DOWNLOAD_BYTES = 128 * 1024 * 1024

export const ARTIFACT_RUNTIME_MAX_REDIRECTS = 5

export const ARTIFACT_RUNTIME_MAX_FILES = 100_000

export const ARTIFACT_RUNTIME_MAX_TOTAL_BYTES = 4 * 1024 * 1024 * 1024

export const RECEIPT_NAME = 'component-receipt.json'

export const BUNDLE_REVISION_PREFIX = 'artifact-runtime-bundle-sha256-v1:'

export const BUILD_INPUTS_REVISION_PREFIX = 'artifact-runtime-build-inputs-sha256-v1:'

const SUPPORTED_PLATFORMS = new Set(['darwin', 'linux', 'win32'])

const SUPPORTED_ARCHITECTURES = new Set(['arm64', 'x64'])

const ALLOWED_DOWNLOAD_HOSTS = new Set([
  'github.com',
  'raw.githubusercontent.com',
  'release-assets.githubusercontent.com',
  'objects.githubusercontent.com',
  'nodejs.org',
  'files.pythonhosted.org',
  'pypi.org'
])

const SHA256_PATTERN = /^[a-f0-9]{64}$/

export const SAFE_ENVIRONMENT = Object.freeze({
  PIP_DISABLE_PIP_VERSION_CHECK: '1',
  PIP_NO_INPUT: '1',
  PYTHONDONTWRITEBYTECODE: '1',
  PYTHONNOUSERSITE: '1',
  PYTHONUTF8: '1'
})

const SCRIPT_DIRECTORY = resolve(dirname(fileURLToPath(import.meta.url)), '..')

export const REPOSITORY_ROOT = resolve(SCRIPT_DIRECTORY, '..')

export const DEFAULT_MANIFEST_PATH = join(
  REPOSITORY_ROOT,
  'resources',
  'artifact-runtime-manifest.json'
)

export const DEFAULT_OUTPUT_DIRECTORY = join(
  REPOSITORY_ROOT,
  '.cache',
  'artifact-runtime',
  'current'
)

export const DEFAULT_DOWNLOAD_DIRECTORY = join(
  REPOSITORY_ROOT,
  '.cache',
  'artifact-runtime',
  'downloads'
)

export const NODE_WORKSPACE_PACKAGE = join(REPOSITORY_ROOT, 'packages', 'artifact-runtime-node')

export const NODE_PACKAGE_EVIDENCE_SOURCE = join(
  REPOSITORY_ROOT,
  'resources',
  'artifact-runtime-node-package-evidence.json'
)

export const NODE_PACKAGE_EVIDENCE_TARGET = 'dependencies/node/node-package-evidence.json'

export const NODE_BOOTSTRAP_SOURCE = join(
  REPOSITORY_ROOT,
  'resources',
  'artifact-runtime',
  'node-bootstrap.mjs'
)

export const NODE_LOADER_SOURCE = join(
  REPOSITORY_ROOT,
  'resources',
  'artifact-runtime',
  'node-loader.mjs'
)

export const PRESENTATION_SDK_SOURCE = join(
  REPOSITORY_ROOT,
  'resources',
  'artifact-runtime',
  'presentation-sdk.mjs'
)

export const BUILD_INPUT_RELATIVE_PATHS = Object.freeze({
  builder: 'scripts/prepare-artifact-runtime.mjs',
  artifactRuntimeContract: 'scripts/artifact-runtime/contract.mjs',
  artifactRuntimeFilesystem: 'scripts/artifact-runtime/filesystem.mjs',
  artifactRuntimeArchive: 'scripts/artifact-runtime/archive.mjs',
  artifactRuntimeNodeDependencies: 'scripts/artifact-runtime/node-dependencies.mjs',
  artifactRuntimePythonRuntime: 'scripts/artifact-runtime/python-runtime.mjs',
  artifactRuntimeLegalEvidence: 'scripts/artifact-runtime/legal-evidence.mjs',
  artifactRuntimeReceipt: 'scripts/artifact-runtime/receipt.mjs',
  artifactRuntimeSigning: 'scripts/artifact-runtime/signing.mjs',
  artifactRuntimePrepare: 'scripts/artifact-runtime/prepare.mjs',
  frozenMachOSigning: 'scripts/frozen-macho-signing.mjs',
  nodeBootstrap: 'resources/artifact-runtime/node-bootstrap.mjs',
  nodeLoader: 'resources/artifact-runtime/node-loader.mjs',
  presentationSdk: 'resources/artifact-runtime/presentation-sdk.mjs',
  nodePackageManifest: 'packages/artifact-runtime-node/package.json',
  nodePackageEvidence: 'resources/artifact-runtime-node-package-evidence.json',
  pptxgenjsPatch: 'patches/pptxgenjs@4.0.1.patch',
  pnpmLockfile: 'pnpm-lock.yaml',
  pythonRequirements: 'resources/artifact-runtime-python-requirements.txt',
  pdfRuntimeCli: 'crates/core/src/command/pdf_runtime_cli.py'
})

export const BUILD_INPUT_SOURCE_PATHS = Object.freeze(
  Object.fromEntries(
    Object.entries(BUILD_INPUT_RELATIVE_PATHS).map(([id, path]) => [
      id,
      join(REPOSITORY_ROOT, ...path.split('/'))
    ])
  )
)

// These packages are exact, reviewed exceptions because their published archive does not contain
// a conventional LICENSE/COPYING/NOTICE file. A new version, changed declaration, or missing
// evidence file fails closed. `buffers@0.1.1` is intentionally NOASSERTION: its npm archive has no
// license declaration at all, and the exception is made visible in the shipped audit inventory
// instead of inventing a license expression.
export const NODE_METADATA_LICENSE_EVIDENCE_ALLOWLIST = Object.freeze({
  'binary@0.3.0': Object.freeze({
    declaredLicense: 'MIT',
    licenseExpression: 'MIT',
    evidenceFiles: Object.freeze(['README.markdown']),
    review: 'package metadata and the shipped README declare MIT'
  }),
  'buffers@0.1.1': Object.freeze({
    declaredLicense: null,
    licenseExpression: 'NOASSERTION',
    evidenceFiles: Object.freeze(['package.json']),
    review: 'exact reviewed exception: the published archive declares no license'
  }),
  'chainsaw@0.1.0': Object.freeze({
    declaredLicense: 'MIT/X11',
    licenseExpression: 'MIT',
    evidenceFiles: Object.freeze(['package.json']),
    review: 'package metadata uses the legacy MIT/X11 identifier'
  }),
  'hash.js@1.1.7': Object.freeze({
    declaredLicense: 'MIT',
    licenseExpression: 'MIT',
    evidenceFiles: Object.freeze(['README.md']),
    review: 'package metadata and the shipped README contain the MIT grant'
  }),
  'isarray@1.0.0': Object.freeze({
    declaredLicense: 'MIT',
    licenseExpression: 'MIT',
    evidenceFiles: Object.freeze(['README.md']),
    review: 'package metadata and the shipped README contain the MIT grant'
  }),
  'saxes@5.0.1': Object.freeze({
    declaredLicense: 'ISC',
    licenseExpression: 'ISC',
    evidenceFiles: Object.freeze(['package.json']),
    review: 'package metadata declares ISC'
  })
})

export const PYTHON_METADATA_LICENSE_EVIDENCE_ALLOWLIST = Object.freeze({
  'et-xmlfile@2.0.0': Object.freeze({
    declaredLicense: 'MIT',
    licenseExpression: 'MIT',
    review: 'wheel METADATA declares MIT and its OSI-approved MIT classifier'
  }),
  'openpyxl@3.1.5': Object.freeze({
    declaredLicense: 'MIT',
    licenseExpression: 'MIT',
    review: 'wheel METADATA declares MIT and its OSI-approved MIT classifier'
  }),
  'pdfplumber@0.11.9': Object.freeze({
    declaredLicense: null,
    licenseExpression: 'MIT',
    review: 'wheel ships the upstream MIT LICENSE.txt while METADATA omits a license field'
  }),
  'pypdfium2@5.12.1': Object.freeze({
    declaredLicense: 'BSD-3-Clause, Apache-2.0, dependency licenses',
    licenseExpression: 'NOASSERTION',
    review:
      'wheel ships Apache-2.0, BSD-3-Clause, CC-BY-4.0, PDFium, and build-dependency license evidence without one aggregate SPDX expression'
  }),
  'reportlab@4.4.9': Object.freeze({
    declaredLicense:
      'BSD license (see license.txt for details), Copyright (c) 2000-2025, ReportLab Inc.',
    licenseExpression: 'BSD-3-Clause',
    review: 'wheel ships the ReportLab BSD license and METADATA identifies it as BSD'
  })
})

export const LICENSE_FILE_PATTERN = /^(licen[cs]e|copying|notice|copyright)([._-].*)?$/i

export const MAX_LEGAL_EVIDENCE_FILES_PER_PACKAGE = 64

export const MAX_LEGAL_EVIDENCE_FILE_BYTES = 2 * 1024 * 1024

export const MAX_NODE_PACKAGE_EVIDENCE_BYTES = 16 * 1024 * 1024

export const MAX_NODE_PACKAGE_FILES = 50_000

export const MAX_NODE_PACKAGE_BYTES = 1024 * 1024 * 1024

export function plainObject(value, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label} must be an object`)
  }
  return value
}

export function exactKeys(value, keys, label) {
  const actual = Object.keys(value).sort()
  const expected = [...keys].sort()
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index])) {
    throw new Error(`${label} must contain exactly: ${expected.join(', ')}`)
  }
}

export function nonEmptyString(value, label) {
  if (typeof value !== 'string' || value.length === 0 || value.trim() !== value) {
    throw new Error(`${label} must be a non-empty, trimmed string`)
  }
  return value
}

function positiveInteger(value, label) {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new Error(`${label} must be a positive safe integer`)
  }
  return value
}

export function sha256(value, label) {
  const digest = nonEmptyString(value, label)
  if (!SHA256_PATTERN.test(digest)) {
    throw new Error(`${label} must be a lowercase SHA-256 digest`)
  }
  return digest
}

export function canonicalRelativePath(value, label) {
  const path = nonEmptyString(value, label)
  if (
    path.length > 1024 ||
    path.includes('\\') ||
    path.includes('\0') ||
    path.startsWith('/') ||
    path.split('/').some((part) => part.length === 0 || part === '.' || part === '..')
  ) {
    throw new Error(`${label} must be a canonical relative path`)
  }
  return path
}

export function validateArtifactRuntimeDownloadUrl(value, label = 'download URL') {
  const raw = nonEmptyString(value, label)
  let url
  try {
    url = new URL(raw)
  } catch {
    throw new Error(`${label} is not a valid URL`)
  }
  if (url.protocol !== 'https:') {
    throw new Error(`${label} must use HTTPS`)
  }
  if (url.username || url.password || url.port) {
    throw new Error(`${label} cannot contain credentials or a custom port`)
  }
  if (!ALLOWED_DOWNLOAD_HOSTS.has(url.hostname.toLowerCase())) {
    throw new Error(`${label} host is not allowlisted: ${url.hostname}`)
  }
  return url
}

function validateAsset(value, label, { node = false } = {}) {
  const asset = plainObject(value, label)
  const allowedKeys = node
    ? asset.format === 'tar.gz'
      ? ['format', 'archiveRoot', 'url', 'size', 'sha256']
      : ['format', 'url', 'size', 'sha256']
    : ['url', 'size', 'sha256']
  exactKeys(asset, allowedKeys, label)
  if (node && !['tar.gz', 'executable'].includes(asset.format)) {
    throw new Error(`${label}.format must be tar.gz or executable`)
  }
  const size = positiveInteger(asset.size, `${label}.size`)
  if (size > ARTIFACT_RUNTIME_MAX_DOWNLOAD_BYTES) {
    throw new Error(`${label}.size exceeds the 128 MiB component download limit`)
  }
  return Object.freeze({
    ...(node ? { format: asset.format } : {}),
    ...(asset.archiveRoot
      ? { archiveRoot: canonicalRelativePath(asset.archiveRoot, `${label}.archiveRoot`) }
      : {}),
    url: validateArtifactRuntimeDownloadUrl(asset.url, `${label}.url`).href,
    size,
    sha256: sha256(asset.sha256, `${label}.sha256`)
  })
}

function validateArchivedToolAsset(value, label) {
  const asset = plainObject(value, label)
  exactKeys(asset, ['format', 'archiveRoot', 'url', 'size', 'sha256'], label)
  if (!['tar.gz', 'zip'].includes(asset.format)) {
    throw new Error(`${label}.format must be tar.gz or zip`)
  }
  const size = positiveInteger(asset.size, `${label}.size`)
  if (size > ARTIFACT_RUNTIME_MAX_DOWNLOAD_BYTES) {
    throw new Error(`${label}.size exceeds the 128 MiB component download limit`)
  }
  return Object.freeze({
    format: asset.format,
    archiveRoot: canonicalRelativePath(asset.archiveRoot, `${label}.archiveRoot`),
    url: validateArtifactRuntimeDownloadUrl(asset.url, `${label}.url`).href,
    size,
    sha256: sha256(asset.sha256, `${label}.sha256`)
  })
}

function validateExecutableMap(value, label) {
  const map = plainObject(value, label)
  exactKeys(map, ['unix', 'win32'], label)
  return Object.freeze({
    unix: canonicalRelativePath(map.unix, `${label}.unix`),
    win32: canonicalRelativePath(map.win32, `${label}.win32`)
  })
}

function validateDependencies(value, expected, label) {
  if (!Array.isArray(value) || value.length !== expected.length) {
    throw new Error(`${label} must contain exactly ${expected.length} pinned dependencies`)
  }
  const dependencies = value.map((entry, index) => {
    const dependency = plainObject(entry, `${label}[${index}]`)
    exactKeys(dependency, ['name', 'version', 'identityFile'], `${label}[${index}]`)
    return Object.freeze({
      name: nonEmptyString(dependency.name, `${label}[${index}].name`),
      version: nonEmptyString(dependency.version, `${label}[${index}].version`),
      identityFile: canonicalRelativePath(
        dependency.identityFile,
        `${label}[${index}].identityFile`
      )
    })
  })
  const actual = new Map(dependencies.map((entry) => [entry.name.toLowerCase(), entry.version]))
  for (const [name, version] of expected) {
    if (actual.get(name) !== version) {
      throw new Error(`${label} must pin ${name}@${version}`)
    }
  }
  return Object.freeze(dependencies)
}

function validateTargetAssets(value, label, options) {
  return validateTargetAssetsWith(value, label, (asset, assetLabel) =>
    validateAsset(asset, assetLabel, options)
  )
}

function validateTargetAssetsWith(value, label, validateTarget) {
  const assets = plainObject(value, label)
  const targets = []
  for (const platform of SUPPORTED_PLATFORMS) {
    for (const arch of SUPPORTED_ARCHITECTURES) {
      targets.push(`${platform}-${arch}`)
    }
  }
  exactKeys(assets, targets, label)
  return Object.freeze(
    Object.fromEntries(
      targets.map((target) => [target, validateTarget(assets[target], `${label}.${target}`)])
    )
  )
}

function validateBuildInputs(value) {
  const inputs = plainObject(value, 'manifest.buildInputs')
  const ids = Object.keys(BUILD_INPUT_RELATIVE_PATHS)
  exactKeys(inputs, ids, 'manifest.buildInputs')
  return Object.freeze(
    Object.fromEntries(
      ids.map((id) => {
        const descriptor = plainObject(inputs[id], `manifest.buildInputs.${id}`)
        exactKeys(descriptor, ['path', 'sha256'], `manifest.buildInputs.${id}`)
        const path = canonicalRelativePath(descriptor.path, `manifest.buildInputs.${id}.path`)
        if (path !== BUILD_INPUT_RELATIVE_PATHS[id]) {
          throw new Error(
            `manifest.buildInputs.${id}.path must remain ${BUILD_INPUT_RELATIVE_PATHS[id]}`
          )
        }
        return [
          id,
          Object.freeze({
            path,
            sha256: sha256(descriptor.sha256, `manifest.buildInputs.${id}.sha256`)
          })
        ]
      })
    )
  )
}

export function validateArtifactRuntimeManifest(value) {
  const manifest = plainObject(value, 'manifest')
  exactKeys(
    manifest,
    ['schemaVersion', 'providerId', 'bundleVersion', 'buildInputs', 'node', 'python', 'tools'],
    'manifest'
  )
  if (manifest.schemaVersion !== 4) {
    throw new Error('manifest.schemaVersion must be 4')
  }
  if (manifest.providerId !== 'mycopilot.artifact-runtime') {
    throw new Error('manifest.providerId must be mycopilot.artifact-runtime')
  }
  if (manifest.bundleVersion !== '2026.08.5') {
    throw new Error('artifact runtime bundle must remain pinned to 2026.08.5')
  }
  const buildInputs = validateBuildInputs(manifest.buildInputs)

  const node = plainObject(manifest.node, 'manifest.node')
  exactKeys(
    node,
    [
      'version',
      'executable',
      'packageRoot',
      'bootstrap',
      'loader',
      'presentationSdk',
      'assets',
      'dependencies',
      'licenseFile'
    ],
    'manifest.node'
  )
  if (node.version !== '22.23.1') {
    throw new Error('managed Node must remain pinned to 22.23.1')
  }
  const licenseFile = plainObject(node.licenseFile, 'manifest.node.licenseFile')
  exactKeys(licenseFile, ['target', 'url', 'size', 'sha256'], 'manifest.node.licenseFile')

  const python = plainObject(manifest.python, 'manifest.python')
  exactKeys(
    python,
    [
      'version',
      'release',
      'executable',
      'runtimeHome',
      'requirements',
      'dependencies',
      'assets',
      'source',
      'license'
    ],
    'manifest.python'
  )
  if (python.version !== '3.12.13' || python.release !== '20260610') {
    throw new Error('managed Python must remain pinned to 3.12.13+20260610')
  }

  const tools = plainObject(manifest.tools, 'manifest.tools')
  exactKeys(tools, ['pdfCli', 'ripgrep'], 'manifest.tools')
  const pdfCli = plainObject(tools.pdfCli, 'manifest.tools.pdfCli')
  exactKeys(pdfCli, ['version', 'target'], 'manifest.tools.pdfCli')
  if (pdfCli.version !== '1') throw new Error('managed PDF CLI must remain pinned to version 1')
  const ripgrep = plainObject(tools.ripgrep, 'manifest.tools.ripgrep')
  exactKeys(
    ripgrep,
    ['version', 'executable', 'assets', 'licenseFiles', 'source', 'license'],
    'manifest.tools.ripgrep'
  )
  if (ripgrep.version !== '15.1.0') {
    throw new Error('managed ripgrep must remain pinned to 15.1.0')
  }
  if (!Array.isArray(ripgrep.licenseFiles) || ripgrep.licenseFiles.length !== 3) {
    throw new Error('managed ripgrep must declare exactly three license evidence files')
  }
  const licenseFiles = ripgrep.licenseFiles.map((value, index) => {
    const descriptor = plainObject(value, `manifest.tools.ripgrep.licenseFiles[${index}]`)
    exactKeys(descriptor, ['source', 'target'], `manifest.tools.ripgrep.licenseFiles[${index}]`)
    return Object.freeze({
      source: canonicalRelativePath(
        descriptor.source,
        `manifest.tools.ripgrep.licenseFiles[${index}].source`
      ),
      target: canonicalRelativePath(
        descriptor.target,
        `manifest.tools.ripgrep.licenseFiles[${index}].target`
      )
    })
  })

  return Object.freeze({
    schemaVersion: 4,
    providerId: manifest.providerId,
    bundleVersion: manifest.bundleVersion,
    buildInputs,
    node: Object.freeze({
      version: node.version,
      executable: validateExecutableMap(node.executable, 'manifest.node.executable'),
      packageRoot: canonicalRelativePath(node.packageRoot, 'manifest.node.packageRoot'),
      bootstrap: canonicalRelativePath(node.bootstrap, 'manifest.node.bootstrap'),
      loader: canonicalRelativePath(node.loader, 'manifest.node.loader'),
      presentationSdk: canonicalRelativePath(node.presentationSdk, 'manifest.node.presentationSdk'),
      assets: validateTargetAssets(node.assets, 'manifest.node.assets', { node: true }),
      dependencies: validateDependencies(
        node.dependencies,
        [
          ['docx', '9.6.1'],
          ['exceljs', '4.4.0'],
          ['pptxgenjs', '4.0.1']
        ],
        'manifest.node.dependencies'
      ),
      licenseFile: Object.freeze({
        target: canonicalRelativePath(licenseFile.target, 'manifest.node.licenseFile.target'),
        url: validateArtifactRuntimeDownloadUrl(licenseFile.url, 'manifest.node.licenseFile.url')
          .href,
        size: positiveInteger(licenseFile.size, 'manifest.node.licenseFile.size'),
        sha256: sha256(licenseFile.sha256, 'manifest.node.licenseFile.sha256')
      })
    }),
    python: Object.freeze({
      version: python.version,
      release: python.release,
      executable: validateExecutableMap(python.executable, 'manifest.python.executable'),
      runtimeHome: canonicalRelativePath(python.runtimeHome, 'manifest.python.runtimeHome'),
      requirements: canonicalRelativePath(python.requirements, 'manifest.python.requirements'),
      dependencies: validateDependencies(
        python.dependencies,
        [
          ['numpy', '2.5.2'],
          ['openpyxl', '3.1.5'],
          ['pandas', '3.0.5'],
          ['pdfplumber', '0.11.9'],
          ['pypdf', '6.15.0'],
          ['pypdfium2', '5.12.1'],
          ['python-docx', '1.2.0'],
          ['python-pptx', '1.0.2'],
          ['reportlab', '4.4.9'],
          ['xlsxwriter', '3.2.9']
        ],
        'manifest.python.dependencies'
      ),
      assets: validateTargetAssets(python.assets, 'manifest.python.assets'),
      source: validateArtifactRuntimeDownloadUrl(python.source, 'manifest.python.source').href,
      license: nonEmptyString(python.license, 'manifest.python.license')
    }),
    tools: Object.freeze({
      pdfCli: Object.freeze({
        version: pdfCli.version,
        target: canonicalRelativePath(pdfCli.target, 'manifest.tools.pdfCli.target')
      }),
      ripgrep: Object.freeze({
        version: ripgrep.version,
        executable: validateExecutableMap(ripgrep.executable, 'manifest.tools.ripgrep.executable'),
        assets: validateTargetAssetsWith(
          ripgrep.assets,
          'manifest.tools.ripgrep.assets',
          validateArchivedToolAsset
        ),
        licenseFiles: Object.freeze(licenseFiles),
        source: validateArtifactRuntimeDownloadUrl(ripgrep.source, 'manifest.tools.ripgrep.source')
          .href,
        license: nonEmptyString(ripgrep.license, 'manifest.tools.ripgrep.license')
      })
    })
  })
}

export async function loadArtifactRuntimeManifest(manifestPath = DEFAULT_MANIFEST_PATH) {
  let parsed
  try {
    parsed = JSON.parse(await readFile(manifestPath, 'utf8'))
  } catch (error) {
    throw new Error(`Artifact runtime manifest is not valid JSON: ${error.message}`, {
      cause: error
    })
  }
  const manifest = validateArtifactRuntimeManifest(parsed)
  await verifyArtifactRuntimeBuildInputs(manifest)
  return manifest
}

export async function verifyArtifactRuntimeBuildInputs(manifest) {
  for (const [id, descriptor] of Object.entries(manifest.buildInputs)) {
    await verifyPinnedLocalFile(
      BUILD_INPUT_SOURCE_PATHS[id],
      descriptor.sha256,
      `Artifact Runtime build input ${id}`
    )
  }
}

export function selectArtifactRuntimeAssets(
  manifest,
  platform = process.platform,
  arch = process.arch
) {
  if (!SUPPORTED_PLATFORMS.has(platform) || !SUPPORTED_ARCHITECTURES.has(arch)) {
    throw new Error(`Managed Artifact Runtime is not packaged for ${platform}-${arch}`)
  }
  const target = `${platform}-${arch}`
  return Object.freeze({
    target,
    node: manifest.node.assets[target],
    python: manifest.python.assets[target],
    ripgrep: manifest.tools.ripgrep.assets[target]
  })
}
