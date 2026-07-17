import { describe, expect, it } from 'vitest'
import { GitReviewPayloadCacheBudget } from '../gitReviewPayloadCacheBudget'

describe('GitReviewPayloadCacheBudget', () => {
  it('evicts least-recently-demanded files by count', () => {
    const budget = new GitReviewPayloadCacheBudget({ maxCharacters: 100, maxFiles: 2 })
    expect(budget.record('first', 10)).toEqual([])
    expect(budget.record('second', 10)).toEqual([])
    budget.touch('first')

    expect(budget.record('third', 10)).toEqual(['second'])
  })

  it('evicts enough entries to satisfy the aggregate character budget', () => {
    const budget = new GitReviewPayloadCacheBudget({ maxCharacters: 10, maxFiles: 10 })
    budget.record('first', 4)
    budget.record('second', 4)

    expect(budget.record('third', 6)).toEqual(['first'])
    expect(budget.record('oversized', 11)).toEqual(['second', 'third', 'oversized'])
  })

  it('replaces an existing entry without double-counting it', () => {
    const budget = new GitReviewPayloadCacheBudget({ maxCharacters: 10, maxFiles: 2 })
    budget.record('first', 8)

    expect(budget.record('first', 3)).toEqual([])
    expect(budget.record('second', 7)).toEqual([])
  })

  it('pins the hot working set and trims it after demand moves away', () => {
    const budget = new GitReviewPayloadCacheBudget({ maxCharacters: 10, maxFiles: 2 })
    budget.record('selected', 8)
    budget.record('visible', 8, new Set(['selected', 'visible']))

    expect(budget.trim(new Set(['selected', 'visible']))).toEqual([])
    expect(budget.trim(new Set(['visible']))).toEqual(['selected'])
  })
})
