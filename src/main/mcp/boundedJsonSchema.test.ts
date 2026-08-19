import { describe, expect, it } from 'vitest'

import { matchesBoundedJsonSchema } from './boundedJsonSchema'

const limits = {
  maxDepth: 16,
  maxNodes: 256,
  maxArrayItems: 32,
  maxObjectProperties: 32
}

describe('matchesBoundedJsonSchema', () => {
  it('preserves required, nested items and fail-closed object properties', () => {
    const schema = {
      type: 'object',
      properties: {
        fields: {
          type: 'array',
          items: {
            type: 'object',
            properties: { name: { type: 'string' }, value: { type: 'string' } },
            required: ['name', 'value'],
            additionalProperties: false
          }
        }
      },
      required: ['fields'],
      additionalProperties: false
    }

    expect(matchesBoundedJsonSchema({ fields: [{ name: 'q', value: 'x' }] }, schema, limits)).toBe(
      true
    )
    expect(matchesBoundedJsonSchema({ fields: [{ name: 'q' }] }, schema, limits)).toBe(false)
    expect(
      matchesBoundedJsonSchema(
        { fields: [{ name: 'q', value: 'x', secret: true }] },
        schema,
        limits
      )
    ).toBe(false)
    expect(matchesBoundedJsonSchema({ fields: [], extra: true }, schema, limits)).toBe(false)
  })

  it('supports composition, const, nullable types and tuple schemas without coercion', () => {
    const schema = {
      oneOf: [
        { type: 'object', properties: { kind: { const: 'point' } }, required: ['kind'] },
        { type: 'null' }
      ]
    }
    expect(matchesBoundedJsonSchema({ kind: 'point' }, schema, limits)).toBe(true)
    expect(matchesBoundedJsonSchema(null, schema, limits)).toBe(true)
    expect(matchesBoundedJsonSchema({ kind: 'other' }, schema, limits)).toBe(false)

    expect(
      matchesBoundedJsonSchema(
        ['x', 2],
        { type: 'array', prefixItems: [{ type: 'string' }, { type: 'integer' }], items: false },
        limits
      )
    ).toBe(true)
    expect(
      matchesBoundedJsonSchema(
        ['x', '2'],
        { type: 'array', prefixItems: [{ type: 'string' }, { type: 'integer' }], items: false },
        limits
      )
    ).toBe(false)
  })

  it('enforces enum, bounds, propertyNames and global resource limits', () => {
    const schema = {
      type: 'object',
      propertyNames: { type: 'string', pattern: '^[a-z]+$' },
      additionalProperties: { type: 'integer', minimum: 0, maximum: 10 }
    }
    expect(matchesBoundedJsonSchema({ safe: 3 }, schema, limits)).toBe(true)
    expect(matchesBoundedJsonSchema({ Unsafe: 3 }, schema, limits)).toBe(false)
    expect(matchesBoundedJsonSchema({ safe: 11 }, schema, limits)).toBe(false)
    expect(
      matchesBoundedJsonSchema(
        Array.from({ length: 33 }, () => 1),
        { type: 'array', items: { type: 'number' } },
        limits
      )
    ).toBe(false)
  })
})
