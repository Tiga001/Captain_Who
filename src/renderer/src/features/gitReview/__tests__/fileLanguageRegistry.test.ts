import { describe, expect, it } from 'vitest'
import {
  resolveGitReviewFileLanguageDescriptor,
  resolveGitReviewSyntaxLanguage
} from '../syntax/fileLanguageRegistry'

describe('resolveGitReviewSyntaxLanguage', () => {
  it.each([
    ['Dockerfile', 'docker'],
    ['infra/Containerfile', 'docker'],
    ['CMakeLists.txt', 'cmake'],
    ['config/.env', 'dotenv'],
    ['.gitmodules', 'ini'],
    ['Cargo.lock', 'toml'],
    ['pnpm-lock.yaml', 'yaml'],
    ['COMMIT_EDITMSG', 'git-commit'],
    ['git-rebase-todo', 'git-rebase'],
    ['CODEOWNERS', 'codeowners'],
    ['README', 'markdown'],
    ['Jenkinsfile', 'groovy'],
    ['Justfile', 'just']
  ] as const)('resolves the exact file name %s as %s', (path, languageId) => {
    expect(resolveGitReviewSyntaxLanguage(path)).toBe(languageId)
  })

  it.each([
    ['src/button.component.html', 'angular-html'],
    ['src/button.component.ts', 'angular-ts'],
    ['types/runtime.d.ts', 'typescript'],
    ['types/runtime.d.mts', 'typescript'],
    ['views/account.blade.php', 'blade'],
    ['templates/widget.vue.html', 'vue-html']
  ] as const)('prefers the compound suffix in %s over its simple extension', (path, languageId) => {
    expect(resolveGitReviewSyntaxLanguage(path)).toBe(languageId)
  })

  it.each([
    ['src/App.TSX', 'tsx'],
    ['src/main.rs', 'rust'],
    ['scripts/release.zsh', 'shellscript'],
    ['schema/service.proto', 'proto'],
    ['assets/logo.svg', 'xml'],
    ['config/app.tfvars', 'terraform'],
    ['docs/guide.mdx', 'mdx'],
    ['data/events.jsonl', 'jsonl']
  ] as const)('resolves the simple extension in %s as %s', (path, languageId) => {
    expect(resolveGitReviewSyntaxLanguage(path)).toBe(languageId)
  })

  it.each([
    ['config/.env.production', 'dotenv'],
    ['containers/Dockerfile.dev', 'docker'],
    ['build/Makefile.release', 'make'],
    ['configs/tsconfig.renderer.json', 'jsonc'],
    ['configs/JSCONFIG.TEST.JSON', 'jsonc']
  ] as const)('resolves the recognized file-name family %s as %s', (path, languageId) => {
    expect(resolveGitReviewSyntaxLanguage(path)).toBe(languageId)
  })

  it('normalizes Windows separators and file-name casing without altering the contract', () => {
    expect(resolveGitReviewSyntaxLanguage('src\\features\\Panel.TSX')).toBe('tsx')
    expect(resolveGitReviewSyntaxLanguage('infra\\DOCKERFILE')).toBe('docker')
  })

  it.each(['NOTICE', 'archive.bin', 'unknown.custom-language', '', 'folder/'])(
    'falls back to text for %j',
    (path) => {
      expect(resolveGitReviewSyntaxLanguage(path)).toBe('text')
    }
  )
})

describe('resolveGitReviewFileLanguageDescriptor', () => {
  it('keeps rename paths and resolves the old and new side independently', () => {
    expect(
      resolveGitReviewFileLanguageDescriptor({
        newPath: 'scripts/worker.py',
        oldPath: 'src/worker.ts'
      })
    ).toEqual({
      newLanguageId: 'python',
      newPath: 'scripts/worker.py',
      oldLanguageId: 'typescript',
      oldPath: 'src/worker.ts'
    })
  })

  it('returns text independently for an unknown side without degrading the known side', () => {
    expect(
      resolveGitReviewFileLanguageDescriptor({
        newPath: 'src/module.rs',
        oldPath: 'src/module.unknown'
      })
    ).toMatchObject({ newLanguageId: 'rust', oldLanguageId: 'text' })
  })
})
