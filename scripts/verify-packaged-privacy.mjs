/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { createHash } from 'node:crypto'
import { createReadStream } from 'node:fs'
import { lstat, readdir, readlink } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { homedir, userInfo } from 'node:os'
import { isAbsolute, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { packagedMacApplicationPaths } from './verify-packaged-macos-signatures.mjs'

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)))
const requireFromBuilder = createRequire(import.meta.resolve('electron-builder/package.json'))
const APP_ASAR_RELATIVE_PATH = 'Contents/Resources/app.asar'
const ASAR_CREDENTIALED_URL_FIXTURES = new Map([
  [
    'node_modules/zod/src/v4/classic/tests/string.test.ts',
    new Set(['https://anonymous:flabada@developer.mozilla.org/en-US/docs/Web/API/URL/password'])
  ]
])
// These exact dummy credentials ship in pinned upstream runtime/parser fixtures. An actual value
// cannot inherit the exception by changing either its full URL or its packaged file.
const PACKAGED_CREDENTIALED_URL_FIXTURES = new Map([
  [
    'Contents/Frameworks/Electron Framework.framework/Versions/A/Electron Framework',
    new Set(['http://a@b', 'http://a@b?@c', 'https://user:pass@host/'])
  ],
  [
    'Contents/Resources/components/office-renderer/browser/chrome-headless-shell-mac-arm64/chrome-headless-shell',
    new Set(['https://user:pass@host/'])
  ],
  [
    'Contents/Resources/components/office-renderer/browser/chrome-headless-shell-mac-x64/chrome-headless-shell',
    new Set(['https://user:pass@host/'])
  ],
  [
    'Contents/Resources/components/artifact-runtime/dependencies/node/bin/node',
    new Set(['http://a@b', 'http://a@b?@c'])
  ],
  [
    'Contents/Resources/components/artifact-runtime/dependencies/node/node_modules/docx/node_modules/@types/node/http.d.ts',
    new Set(['http://abc:xyz@example.com'])
  ],
  [
    'Contents/Resources/components/artifact-runtime/dependencies/node/node_modules/docx/node_modules/@types/node/https.d.ts',
    new Set(['https://abc:xyz@example.com'])
  ],
  [
    'Contents/Resources/components/artifact-runtime/dependencies/node/node_modules/docx/node_modules/@types/node/url.d.ts',
    new Set(['https://a:b@xn--g6w251d/?abc#foo'])
  ],
  [
    'Contents/Resources/components/artifact-runtime/dependencies/node/node_modules/exceljs/node_modules/fast-csv/node_modules/@fast-csv/format/node_modules/@types/node/ts4.8/url.d.ts',
    new Set(['https://a:b@xn--g6w251d/?abc#foo'])
  ],
  [
    'Contents/Resources/components/artifact-runtime/dependencies/node/node_modules/exceljs/node_modules/fast-csv/node_modules/@fast-csv/format/node_modules/@types/node/url.d.ts',
    new Set(['https://a:b@xn--g6w251d/?abc#foo'])
  ],
  [
    'Contents/Resources/components/artifact-runtime/dependencies/node/node_modules/exceljs/node_modules/fast-csv/node_modules/@fast-csv/parse/node_modules/@types/node/ts4.8/url.d.ts',
    new Set(['https://a:b@xn--g6w251d/?abc#foo'])
  ],
  [
    'Contents/Resources/components/artifact-runtime/dependencies/node/node_modules/exceljs/node_modules/fast-csv/node_modules/@fast-csv/parse/node_modules/@types/node/url.d.ts',
    new Set(['https://a:b@xn--g6w251d/?abc#foo'])
  ],
  [
    'Contents/Resources/components/artifact-runtime/dependencies/node/node_modules/pptxgenjs/node_modules/@types/node/http.d.ts',
    new Set(['http://abc:xyz@example.com'])
  ],
  [
    'Contents/Resources/components/artifact-runtime/dependencies/node/node_modules/pptxgenjs/node_modules/@types/node/https.d.ts',
    new Set(['https://abc:xyz@example.com'])
  ],
  [
    'Contents/Resources/components/artifact-runtime/dependencies/node/node_modules/pptxgenjs/node_modules/@types/node/url.d.ts',
    new Set([
      'https://123:xyz@example.com/',
      'https://a:b@xn--g6w251d/?abc#foo',
      'https://abc:123@example.com/',
      'https://abc:xyz@example.com'
    ])
  ],
  [
    'Contents/Resources/components/artifact-runtime/dependencies/python/lib/python3.12/site-packages/pip/_internal/req/constructors.py',
    new Set(['http://blahblah@rev#egg=Foobar', 'http://blahblah@rev#subdirectory=subdir'])
  ],
  [
    'Contents/Resources/components/artifact-runtime/dependencies/python/lib/python3.12/site-packages/pip/_vendor/urllib3/util/url.py',
    new Set(['https://username:password@host.com:80/path?query#fragment'])
  ],
  [
    'Contents/Resources/components/artifact-runtime/dependencies/python/lib/tcl9/9.0/http-2.10.1.tm',
    new Set(['http://jschmoe:xyzzy@www.bogus.net:8000/foo/bar.tml?q=foo#changes'])
  ],
  [
    'Contents/Resources/components/word-pdf-renderer/libreoffice/LibreOffice.app/Contents/Frameworks/LibreOfficePython.framework/Versions/3.12/lib/python3.12/site-packages/pip/_internal/req/constructors.py',
    new Set(['http://blahblah@rev#egg=Foobar'])
  ],
  [
    'Contents/Resources/components/word-pdf-renderer/libreoffice/LibreOffice.app/Contents/Frameworks/LibreOfficePython.framework/Versions/3.12/lib/python3.12/site-packages/pip/_vendor/urllib3/util/url.py',
    new Set(['http://username:password@host.com:80/path?query#fragment'])
  ]
])
const CREDENTIALED_URL_PATTERN =
  /https?:\/\/[A-Za-z0-9._~!$&()*+,;=%-]{1,512}(?::[A-Za-z0-9._~!$&()*+,;=:%-]{1,512})?@(?:\[[A-Fa-f0-9:.]{2,64}\]|[A-Za-z0-9.-]{1,253})(?::[0-9]{1,5})?(?:[/?#][A-Za-z0-9._~!$&()*+,;=:@%/?#-]{0,2048})?/gi
const HIGH_CONFIDENCE_SECRET_PATTERNS = Object.freeze([
  /sk-ant-[A-Za-z0-9_-]{24,200}/g,
  /sk-(?:proj-)?[A-Za-z0-9_-]{40,200}/g,
  /github_pat_[A-Za-z0-9_]{24,200}/g,
  /AKIA[0-9A-Z]{16}/g,
  /gh[pousr]_[A-Za-z0-9]{24,200}/g,
  /glpat-[A-Za-z0-9_-]{20,200}/g,
  /AIza[A-Za-z0-9_-]{35}/g,
  /[sr]k_live_[A-Za-z0-9]{20,200}/g,
  /gsk_[A-Za-z0-9]{20,200}/g,
  /xox[baprs]-[A-Za-z0-9-]{20,200}/g,
  /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----[\s\S]{32,8192}-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/g
])
// Pinned vendor binaries contain these exact generated Unicode/font test identifiers. They match
// public token formats byte-for-byte but are fixed data, not credentials. Keep value-level (rather
// than path-level) exceptions so any changed or additional token still fails closed.
const HIGH_CONFIDENCE_SECRET_FIXTURES = new Set([
  'AKIA1JDQACQCATIAFGDQ',
  'AKIAAQAAAAAABAAHAM0A',
  'AKIASACKAKUARCSDCAAI',
  'AKIASACKAKUARCSDCABI',
  'AKIASIHQYCAAXENBQJAA',
  'AKIASIHQYCACHFFDQIMC',
  'AKIAZB0JSCARCRFQALQQ',
  'sk-SKsq-ALsv-SEth-THtr-TRur-PKid-IDuk-UAbe-BYsl-SIet-EElv-LVlt-LTtg-Cyrl-TJfa-IRvi-VNhy-AMaz-Latn-AZeu-EShsb-DEmk-MKtn-ZAxh-ZAzu-ZAaf-ZAka-GEfo-FOhi-INmt-MTse-NOms-MYkk-KZky-KGsw-KEtk-TMuz-Latn-UZtt-RUbn',
  'sk-sksl-sisma-nosma-sesmj-nosmj-sesmn-fisms-fisn-latn-zwso-djso-etso-keso-sosq-alsq-mksq-xksr-cyrl-basr-cyrl-cssr-cyrl-mesr-cyrl-rssr-cyrl-xksr-latn-basr-latn-cssr-latn-mesr-latn-rssr-latn-xkss-szss-zass'
])
// Chromium embeds three fixed public Google API identifiers. Bind their digests to the exact
// frozen executable path so the value is not copied into source and cannot exempt another file.
const PACKAGED_SECRET_FIXTURE_SHA256 = new Map([
  ...[
    'Contents/Resources/components/office-renderer/browser/chrome-headless-shell-mac-arm64/chrome-headless-shell',
    'Contents/Resources/components/office-renderer/browser/chrome-headless-shell-mac-x64/chrome-headless-shell'
  ].map((path) => [
    path,
    new Set([
      '53fe277c7b4839401f5491687cdc9d02e79c3ef4f6b438c29f2f408739c65ed0',
      '76d0a2783815b7cc504688d7507d922871d8bd77ed7fdaa9af78340b8549ecf1',
      '3ad816e16b77c277be31ff3389409f80ba6ab4c2337e3a7021918fe952032e00'
    ])
  ])
])
const SECRET_MARKERS = Object.freeze([
  Buffer.from('sk-', 'ascii'),
  Buffer.from('github_pat_', 'ascii'),
  Buffer.from('AKIA', 'ascii'),
  Buffer.from('gh', 'ascii'),
  Buffer.from('glpat-', 'ascii'),
  Buffer.from('AIza', 'ascii'),
  Buffer.from('sk_live_', 'ascii'),
  Buffer.from('rk_live_', 'ascii'),
  Buffer.from('gsk_', 'ascii'),
  Buffer.from('xox', 'ascii'),
  Buffer.from('-----BEGIN ', 'ascii')
])
const URL_MARKERS = Object.freeze([
  Buffer.from('http://', 'ascii'),
  Buffer.from('https://', 'ascii'),
  Buffer.from('HTTP://', 'ascii'),
  Buffer.from('HTTPS://', 'ascii')
])
const SCAN_OVERLAP_BYTES = 16 * 1024
const SENSITIVE_FILE_NAME_PATTERN =
  /^(?:\.DS_Store|\.env(?:\..+)?|\.netrc|\.npmrc|\.pypirc|.*\.(?:p12|key|sqlite|sqlite3|db|log|history))$/i

function containsUnauthorizedSecret(text, allowedFixtureSha256 = new Set()) {
  for (const pattern of HIGH_CONFIDENCE_SECRET_PATTERNS) {
    pattern.lastIndex = 0
    for (const match of text.matchAll(pattern)) {
      if (HIGH_CONFIDENCE_SECRET_FIXTURES.has(match[0])) continue
      const digest = createHash('sha256').update(match[0]).digest('hex')
      if (!allowedFixtureSha256.has(digest)) return true
    }
  }
  return false
}

function containsUnauthorizedCredentialedUrl(text, allowedFixtures = new Set()) {
  CREDENTIALED_URL_PATTERN.lastIndex = 0
  for (const match of text.matchAll(CREDENTIALED_URL_PATTERN)) {
    if (!allowedFixtures.has(match[0])) return true
  }
  return false
}

async function verifyPackagedFileBytes(
  path,
  {
    relativePath,
    privatePathPrefixes,
    scanCredentialedUrls = true,
    allowedSecretFixtureSha256 = new Set(),
    allowedCredentialedUrls = new Set()
  }
) {
  const maxPrivatePrefixLength = Math.max(
    1,
    ...privatePathPrefixes.map((sequence) => sequence.length)
  )
  const overlap = Math.max(SCAN_OVERLAP_BYTES, maxPrivatePrefixLength - 1)
  let tail = Buffer.alloc(0)
  for await (const chunk of createReadStream(path, { highWaterMark: 1024 * 1024 })) {
    const window = tail.length === 0 ? chunk : Buffer.concat([tail, chunk])
    if (privatePathPrefixes.some((sequence) => window.includes(sequence))) {
      throw new Error(`Packaged application contains a private build identity in ${relativePath}`)
    }
    const mayContainSecret = SECRET_MARKERS.some((marker) => window.includes(marker))
    const mayContainUrl =
      scanCredentialedUrls && URL_MARKERS.some((marker) => window.includes(marker))
    if (mayContainSecret || mayContainUrl) {
      const text = window.toString('latin1')
      if (mayContainSecret && containsUnauthorizedSecret(text, allowedSecretFixtureSha256)) {
        throw new Error(`Packaged application contains a high-confidence secret in ${relativePath}`)
      }
      if (mayContainUrl && containsUnauthorizedCredentialedUrl(text, allowedCredentialedUrls)) {
        throw new Error(`Packaged application contains a credentialed URL in ${relativePath}`)
      }
    }
    tail = window.subarray(Math.max(0, window.length - overlap))
  }
}

async function walkPackagedFiles(root) {
  const pending = [root]
  const files = []
  const symbolicLinks = []
  while (pending.length > 0) {
    const directory = pending.pop()
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      if (SENSITIVE_FILE_NAME_PATTERN.test(entry.name)) {
        throw new Error(`Packaged application contains a sensitive state file: ${entry.name}`)
      }
      if (entry.isSymbolicLink()) {
        symbolicLinks.push({ path, target: await readlink(path) })
        continue
      }
      if (entry.isDirectory()) {
        pending.push(path)
        continue
      }
      const metadata = await lstat(path)
      if (metadata.isFile() && !metadata.isSymbolicLink()) files.push(path)
    }
  }
  return { files, symbolicLinks }
}

function loadAsarApi() {
  return requireFromBuilder('@electron/asar')
}

function verifyAsarCredentialedUrls(asarPath, asarApi) {
  for (const listedEntry of asarApi.listPackage(asarPath)) {
    const entry = listedEntry.replace(/^[/\\]+/, '').replaceAll('\\', '/')
    if (!entry) continue
    const entryName = entry.slice(entry.lastIndexOf('/') + 1)
    if (SENSITIVE_FILE_NAME_PATTERN.test(entryName)) {
      throw new Error(`Packaged ASAR contains a sensitive state file: ${entry}`)
    }
    let metadata
    try {
      metadata = asarApi.statFile(asarPath, entry, false)
    } catch (error) {
      throw new Error(`Packaged ASAR entry metadata is unreadable: ${entry}`, { cause: error })
    }
    if (metadata?.files && typeof metadata.files === 'object') continue
    if (!metadata || !Number.isSafeInteger(metadata.size) || metadata.size < 0) {
      throw new Error(`Packaged ASAR entry is not a regular file: ${entry}`)
    }
    let bytes
    try {
      bytes = asarApi.extractFile(asarPath, entry)
    } catch (error) {
      throw new Error(`Packaged ASAR file is unreadable: ${entry}`, { cause: error })
    }
    if (!Buffer.isBuffer(bytes)) {
      throw new Error(`Packaged ASAR file did not yield bytes: ${entry}`)
    }
    const allowedFixtures = ASAR_CREDENTIALED_URL_FIXTURES.get(entry) ?? new Set()
    if (containsUnauthorizedCredentialedUrl(bytes.toString('latin1'), allowedFixtures)) {
      throw new Error(`Packaged application contains a credentialed URL in ${entry}`)
    }
  }
}

export async function verifyPackagedPrivacy(
  context,
  {
    privatePathPrefixes = [repositoryRoot, homedir(), `${userInfo().username}@`],
    asarApi = loadAsarApi()
  } = {}
) {
  if (
    !Array.isArray(privatePathPrefixes) ||
    privatePathPrefixes.length === 0 ||
    privatePathPrefixes.some((prefix) => typeof prefix !== 'string' || prefix.length === 0)
  ) {
    throw new Error('Packaged privacy verification requires private path prefixes')
  }
  if (
    !asarApi ||
    typeof asarApi.listPackage !== 'function' ||
    typeof asarApi.extractFile !== 'function' ||
    typeof asarApi.statFile !== 'function'
  ) {
    throw new Error('Packaged privacy verification requires an ASAR reader')
  }
  const { app } = packagedMacApplicationPaths(context)
  const prefixBytes = [...new Set(privatePathPrefixes)].map((prefix) => Buffer.from(prefix, 'utf8'))
  const packaged = await walkPackagedFiles(app)
  for (const { path, target } of packaged.symbolicLinks) {
    const relativePath = relative(app, path)
    if (isAbsolute(target)) {
      throw new Error(`Packaged application contains an absolute symbolic link in ${relativePath}`)
    }
    const targetBytes = Buffer.from(target, 'utf8')
    if (prefixBytes.some((prefix) => targetBytes.includes(prefix))) {
      throw new Error(
        `Packaged application symbolic link contains a private build identity in ${relativePath}`
      )
    }
  }
  for (const path of packaged.files) {
    const relativePath = relative(app, path)
    await verifyPackagedFileBytes(path, {
      relativePath,
      privatePathPrefixes: prefixBytes,
      // Packed ASAR files are scanned entry-by-entry below so the one exact upstream fixture can
      // be attributed narrowly. The raw ASAR stream is still scanned here for paths and secrets.
      scanCredentialedUrls: relativePath !== APP_ASAR_RELATIVE_PATH,
      allowedSecretFixtureSha256: PACKAGED_SECRET_FIXTURE_SHA256.get(relativePath) ?? new Set(),
      allowedCredentialedUrls: PACKAGED_CREDENTIALED_URL_FIXTURES.get(relativePath) ?? new Set()
    })
  }

  const asarPath = join(app, 'Contents', 'Resources', 'app.asar')
  verifyAsarCredentialedUrls(asarPath, asarApi)
  return Object.freeze({ app, asarPath })
}
