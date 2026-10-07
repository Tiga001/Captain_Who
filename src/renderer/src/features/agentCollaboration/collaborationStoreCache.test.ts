import { describe, expect, it, vi } from 'vitest'
import { CollaborationStore } from './collaborationStore'
import { CollaborationStoreCache } from './collaborationStoreCache'
import type { CollaborationDataSource } from './collaborationClient'

vi.mock('../../host/hostClient', () => ({ hostClient: { agent: {} } }))

function fixture() {
  const subscribed = new Set<string>()
  const source: CollaborationDataSource = {
    getTree: vi.fn(async () => null),
    listEvents: vi.fn(),
    subscribe: () => () => undefined,
    subscribeResync: () => () => undefined
  }
  const createStore = vi.fn(
    (root: string) =>
      new CollaborationStore(root, {
        ...source,
        subscribe: () => {
          subscribed.add(root)
          return () => {
            subscribed.delete(root)
          }
        }
      })
  )
  return { createStore, subscribed }
}

describe('CollaborationStoreCache', () => {
  it('keeps four recently used roots and releases subscriptions during navigation', () => {
    const { createStore, subscribed } = fixture()
    const cache = new CollaborationStoreCache(createStore)
    const first = cache.get('a')
    cache.activate(first)
    const second = cache.get('b')
    cache.activate(second)
    expect(subscribed).toEqual(new Set(['b']))
    expect(cache.get('a')).toBe(first)
    cache.get('c')
    cache.get('d')
    cache.get('e')
    expect(cache.get('b')).toBe(second) // The active root cannot be evicted.
    expect(cache.get('a')).not.toBe(first)
    cache.release(second)
    expect(subscribed.size).toBe(0)
  })

  it('expires idle checkpoints without starting background subscriptions', () => {
    const { createStore, subscribed } = fixture()
    let now = 0
    const cache = new CollaborationStoreCache(createStore, () => now)
    const first = cache.get('a')
    cache.activate(first)
    cache.release(first)
    now += 5 * 60_000
    expect(cache.get('a')).not.toBe(first)
    expect(subscribed.size).toBe(0)
  })

  it('does not share checkpoints across owner scopes and supports effect cleanup/setup', () => {
    const { createStore, subscribed } = fixture()
    const firstOwner = new CollaborationStoreCache(createStore)
    const secondOwner = new CollaborationStoreCache(createStore)
    const first = firstOwner.get('a')
    expect(secondOwner.get('a')).not.toBe(first)
    firstOwner.activate(first)
    firstOwner.release(first)
    firstOwner.activate(first)
    expect(subscribed).toEqual(new Set(['a']))
    firstOwner.release(first)
  })
})
