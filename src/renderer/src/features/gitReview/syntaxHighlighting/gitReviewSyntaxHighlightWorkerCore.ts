import {
  createHighlighterCore,
  type HighlighterCore,
  type LanguageInput,
  type ThemeRegistration
} from 'shiki/core'
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript'
import { bundledLanguages, bundledLanguagesAlias } from 'shiki/langs'
import {
  assessGitReviewHighlightBudget,
  createPlainGitReviewHighlightResult,
  isGitReviewSyntaxColor,
  normalizeGitReviewLanguage,
  type GitReviewHighlightLine,
  type GitReviewHighlightResult,
  type GitReviewHighlightToken,
  type GitReviewHighlightWorkerInput
} from './gitReviewSyntaxHighlightTypes'

const WORKER_THEME_NAME = 'mycopilot-git-review'

/**
 * This theme is deliberately worker-owned and immutable. Every foreground value is a trusted CSS
 * variable, so syntax results can be applied without accepting arbitrary inline styles.
 */
const WORKER_THEME: ThemeRegistration = {
  bg: 'var(--git-review-syntax-background)',
  fg: 'var(--git-review-syntax-foreground)',
  name: WORKER_THEME_NAME,
  settings: [
    {
      settings: {
        background: 'var(--git-review-syntax-background)',
        fontStyle: '',
        foreground: 'var(--git-review-syntax-foreground)'
      }
    },
    {
      scope: ['comment', 'punctuation.definition.comment'],
      settings: { fontStyle: '', foreground: 'var(--git-review-syntax-comment)' }
    },
    {
      scope: [
        'string',
        'string.quoted',
        'string.template',
        'punctuation.definition.string'
      ],
      settings: { fontStyle: '', foreground: 'var(--git-review-syntax-string)' }
    },
    {
      scope: ['string.regexp', 'constant.other.character-class.regexp'],
      settings: { fontStyle: '', foreground: 'var(--git-review-syntax-regexp)' }
    },
    {
      scope: ['constant.numeric', 'constant.language.boolean', 'constant.language.null'],
      settings: { fontStyle: '', foreground: 'var(--git-review-syntax-number)' }
    },
    {
      scope: [
        'keyword',
        'storage',
        'storage.type',
        'storage.modifier',
        'keyword.operator',
        'keyword.control'
      ],
      settings: { fontStyle: '', foreground: 'var(--git-review-syntax-keyword)' }
    },
    {
      scope: [
        'entity.name.function',
        'meta.function-call entity.name.function',
        'support.function',
        'variable.function'
      ],
      settings: { fontStyle: '', foreground: 'var(--git-review-syntax-function)' }
    },
    {
      scope: [
        'entity.name.type',
        'entity.name.class',
        'entity.name.struct',
        'entity.name.enum',
        'support.type',
        'support.class'
      ],
      settings: { fontStyle: '', foreground: 'var(--git-review-syntax-type)' }
    },
    {
      scope: ['variable', 'variable.other', 'variable.parameter', 'meta.definition.variable'],
      settings: { fontStyle: '', foreground: 'var(--git-review-syntax-variable)' }
    },
    {
      scope: ['constant', 'support.constant', 'entity.name.constant'],
      settings: { fontStyle: '', foreground: 'var(--git-review-syntax-constant)' }
    },
    {
      scope: ['entity.name.tag', 'support.class.component'],
      settings: { fontStyle: '', foreground: 'var(--git-review-syntax-tag)' }
    },
    {
      scope: ['entity.other.attribute-name', 'support.type.property-name'],
      settings: { fontStyle: '', foreground: 'var(--git-review-syntax-attribute)' }
    },
    {
      scope: ['invalid', 'invalid.illegal'],
      settings: { fontStyle: '', foreground: 'var(--git-review-syntax-invalid)' }
    }
  ],
  type: 'dark'
}

const PLAIN_LANGUAGES = new Set(['plain', 'plaintext', 'text', 'txt'])
const canonicalLanguageLoaders = bundledLanguages as Record<string, LanguageInput>
const aliasLanguageLoaders = bundledLanguagesAlias as Record<string, LanguageInput>

let highlighterPromise: Promise<HighlighterCore> | undefined
const loadedLanguageRequests = new Set<string>()

export async function highlightGitReviewCodeInWorker(
  input: GitReviewHighlightWorkerInput
): Promise<GitReviewHighlightResult> {
  const language = normalizeGitReviewLanguage(input.language)
  const normalizedInput = { ...input, language }

  if (input.code.length === 0) {
    return createPlainGitReviewHighlightResult(normalizedInput, 'empty')
  }

  const assessment = assessGitReviewHighlightBudget(input.code, input.budget)
  if (assessment.wholeDocumentFallback) {
    return createPlainGitReviewHighlightResult(normalizedInput, assessment.wholeDocumentFallback)
  }
  if (PLAIN_LANGUAGES.has(language)) {
    return createPlainGitReviewHighlightResult(normalizedInput, 'plain-language')
  }

  const languageLoader = canonicalLanguageLoaders[language] ?? aliasLanguageLoaders[language]
  if (!languageLoader) {
    return createPlainGitReviewHighlightResult(normalizedInput, 'unsupported-language')
  }

  const highlighter = await getWorkerHighlighter()
  if (!loadedLanguageRequests.has(language)) {
    await highlighter.loadLanguage(languageLoader)
    loadedLanguageRequests.add(language)
  }

  const tokenLines = highlighter.codeToTokensBase(input.code, {
    lang: language,
    theme: WORKER_THEME_NAME,
    tokenizeMaxLineLength: input.budget.tokenizeMaxLineLength,
    tokenizeTimeLimit: input.budget.tokenizeTimeLimitMs
  })

  return {
    cacheKey: input.cacheKey,
    language,
    lines: tokenLines.map(mapTokenLine),
    mode: 'highlighted',
    reason: assessment.hasOverlongLine ? 'line-length-budget' : undefined
  }
}

export async function disposeGitReviewSyntaxHighlighter(): Promise<void> {
  const current = highlighterPromise
  highlighterPromise = undefined
  loadedLanguageRequests.clear()
  if (!current) return
  try {
    const highlighter = await current
    highlighter.dispose()
  } catch {
    // Initialization failures have no retained resources to dispose.
  }
}

function getWorkerHighlighter(): Promise<HighlighterCore> {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighterCore({
      engine: createJavaScriptRegexEngine(),
      langs: [],
      themes: [WORKER_THEME]
    }).catch((error: unknown) => {
      highlighterPromise = undefined
      throw error
    })
  }
  return highlighterPromise
}

function mapTokenLine(
  tokens: readonly { color?: string; content: string }[],
  line: number
): GitReviewHighlightLine {
  const mapped: GitReviewHighlightToken[] = []
  let column = 0

  for (const token of tokens) {
    if (token.content.length === 0) continue
    const color = token.color && isGitReviewSyntaxColor(token.color) ? token.color : undefined
    const previous = mapped.at(-1)
    if (previous && previous.color === color && previous.end === column) {
      mapped[mapped.length - 1] = {
        ...previous,
        content: previous.content + token.content,
        end: column + token.content.length
      }
    } else {
      mapped.push({
        color,
        content: token.content,
        end: column + token.content.length,
        start: column
      })
    }
    column += token.content.length
  }

  return { line, tokens: mapped }
}
