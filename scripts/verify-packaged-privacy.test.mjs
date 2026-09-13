/* eslint-disable @typescript-eslint/explicit-function-return-type -- Test fixtures intentionally use compact callbacks. */

import assert from 'node:assert/strict'
import { mkdtemp, mkdir, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'

import { verifyPackagedPrivacy } from './verify-packaged-privacy.mjs'

function context(appOutDir) {
  return {
    electronPlatformName: 'darwin',
    appOutDir,
    packager: { appInfo: { productFilename: 'Captain Who' } }
  }
}

async function fixture() {
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-packaged-privacy-'))
  const app = join(directory, 'Captain Who.app')
  const asarPath = join(app, 'Contents', 'Resources', 'app.asar')
  await mkdir(dirname(asarPath), { recursive: true })
  await writeFile(asarPath, 'safe packaged application')
  await writeFile(join(app, 'Contents', 'Info.plist'), '<plist/>')
  return { directory, app, asarPath }
}

function emptyAsarApi(overrides = {}) {
  return {
    listPackage: () => [],
    statFile: () => ({ size: 0 }),
    extractFile: () => Buffer.alloc(0),
    ...overrides
  }
}

test('packaged privacy gate accepts clean bytes and the one frozen dependency URL fixture', async () => {
  const { directory } = await fixture()
  const asarApi = {
    listPackage: () => ['/node_modules/zod/src/v4/classic/tests/string.test.ts'],
    statFile: () => ({ size: 80_260 }),
    extractFile: () =>
      Buffer.from('https://anonymous:flabada@developer.mozilla.org/en-US/docs/Web/API/URL/password')
  }
  await verifyPackagedPrivacy(context(directory), {
    privatePathPrefixes: ['/private/build-user/project'],
    asarApi
  })
})

test('packaged privacy gate rejects host paths, sensitive state, credentialed URLs, and tokens', async (t) => {
  await t.test('private path', async () => {
    const { directory, app } = await fixture()
    await writeFile(join(app, 'Contents', 'private.bin'), '/private/build-user/project/source.cc')
    await assert.rejects(
      verifyPackagedPrivacy(context(directory), {
        privatePathPrefixes: ['/private/build-user/project'],
        asarApi: emptyAsarApi()
      }),
      /private build identity/
    )
  })

  await t.test('state file', async () => {
    const { directory, app } = await fixture()
    await writeFile(join(app, 'Contents', '.env'), 'TOKEN=private')
    await assert.rejects(
      verifyPackagedPrivacy(context(directory), {
        privatePathPrefixes: ['/private/build-user/project'],
        asarApi: emptyAsarApi()
      }),
      /sensitive state file/
    )
  })

  for (const filename of [
    'account-session.enc',
    'account-license.enc',
    'account-license.enc.tmp'
  ]) {
    await t.test(`encrypted account state: ${filename}`, async () => {
      const { directory, app } = await fixture()
      await writeFile(join(app, 'Contents', filename), Buffer.from([1, 2, 3]))
      await assert.rejects(
        verifyPackagedPrivacy(context(directory), {
          privatePathPrefixes: ['/private/build-user/project'],
          asarApi: emptyAsarApi()
        }),
        /sensitive state file/
      )
    })
  }

  await t.test('absolute private symlink', async () => {
    const { directory, app } = await fixture()
    await symlink(
      '/Users/private-builder/project/config.json',
      join(app, 'Contents', 'Resources', 'private-link')
    )
    await assert.rejects(
      verifyPackagedPrivacy(context(directory), {
        privatePathPrefixes: ['/Users/private-builder'],
        asarApi: emptyAsarApi()
      }),
      /absolute symbolic link/
    )
  })

  await t.test('credentialed URL', async () => {
    const { directory } = await fixture()
    await assert.rejects(
      verifyPackagedPrivacy(context(directory), {
        privatePathPrefixes: ['/private/build-user/project'],
        asarApi: {
          listPackage: () => ['/out/main/index.js'],
          statFile: () => ({ size: 72 }),
          extractFile: () => Buffer.from('https://private-user:private-password@private.invalid/')
        }
      }),
      /credentialed URL/
    )
  })

  await t.test('high-confidence token', async () => {
    const { directory, asarPath } = await fixture()
    await writeFile(asarPath, `const token = "sk-${'A'.repeat(48)}"`)
    await assert.rejects(
      verifyPackagedPrivacy(context(directory), {
        privatePathPrefixes: ['/private/build-user/project'],
        asarApi: emptyAsarApi()
      }),
      /high-confidence secret/
    )
  })

  for (const [name, token] of [
    ['GitHub fine-grained token', `github_pat_${'A1_'.repeat(12)}`],
    ['GitLab token', `glpat-${'B2'.repeat(12)}`],
    ['Google API key', `AIza${'C3_'.repeat(11)}C3`],
    ['Stripe secret key', `sk_live_${'D4'.repeat(12)}`],
    ['Stripe restricted key', `rk_live_${'E5'.repeat(12)}`],
    ['Groq API key', `gsk_${'F6'.repeat(12)}`]
  ]) {
    await t.test(name, async () => {
      const { directory, app } = await fixture()
      await writeFile(join(app, 'Contents', 'Resources', 'runtime-config.json'), token)
      await assert.rejects(
        verifyPackagedPrivacy(context(directory), {
          privatePathPrefixes: ['/private/build-user/project'],
          asarApi: emptyAsarApi()
        }),
        /high-confidence secret.*runtime-config\.json/
      )
    })
  }

  await t.test('secret outside ASAR', async () => {
    const { directory, app } = await fixture()
    await writeFile(
      join(app, 'Contents', 'Resources', 'runtime-config.json'),
      `sk-${'B7'.repeat(24)}`
    )
    await assert.rejects(
      verifyPackagedPrivacy(context(directory), {
        privatePathPrefixes: ['/private/build-user/project'],
        asarApi: emptyAsarApi()
      }),
      /high-confidence secret.*runtime-config\.json/
    )
  })

  await t.test('username-only credential outside ASAR', async () => {
    const { directory, app } = await fixture()
    await writeFile(
      join(app, 'Contents', 'Resources', 'runtime-config.json'),
      'https://private-token@private.invalid/'
    )
    await assert.rejects(
      verifyPackagedPrivacy(context(directory), {
        privatePathPrefixes: ['/private/build-user/project'],
        asarApi: emptyAsarApi()
      }),
      /credentialed URL.*runtime-config\.json/
    )
  })
})

test('packaged privacy gate distinguishes ASAR directories and fails closed on unreadable files', async () => {
  const { directory } = await fixture()
  const extracted = []
  await assert.rejects(
    verifyPackagedPrivacy(context(directory), {
      privatePathPrefixes: ['/private/build-user/project'],
      asarApi: {
        listPackage: () => ['/node_modules', '/out/main/index.js'],
        statFile(_path, entry) {
          return entry === 'node_modules' ? { files: {} } : { size: 10 }
        },
        extractFile(_path, entry) {
          extracted.push(entry)
          throw new Error('corrupt ASAR fixture')
        }
      }
    }),
    /ASAR file is unreadable: out\/main\/index\.js/
  )
  assert.deepEqual(extracted, ['out/main/index.js'])
})

test('packaged privacy gate rejects sensitive state filenames inside ASAR', async () => {
  const { directory } = await fixture()
  const extracted = []
  await assert.rejects(
    verifyPackagedPrivacy(context(directory), {
      privatePathPrefixes: ['/private/build-user/project'],
      asarApi: {
        listPackage: () => ['/out/main/.env'],
        statFile: () => ({ size: 17 }),
        extractFile(_path, entry) {
          extracted.push(entry)
          return Buffer.from('MODE=production')
        }
      }
    }),
    /ASAR contains a sensitive state file: out\/main\/\.env/
  )
  assert.deepEqual(extracted, [])
})

test('the zod exception permits only its exact fake credential URL', async () => {
  const { directory } = await fixture()
  await assert.rejects(
    verifyPackagedPrivacy(context(directory), {
      privatePathPrefixes: ['/private/build-user/project'],
      asarApi: {
        listPackage: () => ['/node_modules/zod/src/v4/classic/tests/string.test.ts'],
        statFile: () => ({ size: 128 }),
        extractFile: () =>
          Buffer.from(
            'https://anonymous:flabada@developer.mozilla.org/en-US/docs/Web/API/URL/password ' +
              'https://anonymous:flabada@private.invalid/'
          )
      }
    }),
    /credentialed URL in node_modules\/zod\/src\/v4\/classic\/tests\/string\.test\.ts/
  )
})

test('ordinary-file exceptions bind both the full fake URL and its exact packaged path', async () => {
  const { directory, app } = await fixture()
  const electronFramework = join(
    app,
    'Contents',
    'Frameworks',
    'Electron Framework.framework',
    'Versions',
    'A',
    'Electron Framework'
  )
  await mkdir(dirname(electronFramework), { recursive: true })
  await writeFile(electronFramework, 'https://user:pass@host/')
  await verifyPackagedPrivacy(context(directory), {
    privatePathPrefixes: ['/private/build-user/project'],
    asarApi: emptyAsarApi()
  })

  await writeFile(electronFramework, 'https://user:pass@host/ https://user:pass@private.invalid/')
  await assert.rejects(
    verifyPackagedPrivacy(context(directory), {
      privatePathPrefixes: ['/private/build-user/project'],
      asarApi: emptyAsarApi()
    }),
    /credentialed URL.*Electron Framework/
  )
})
