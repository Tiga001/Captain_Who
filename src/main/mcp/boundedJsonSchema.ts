export interface BoundedJsonSchemaOptions {
  maxDepth: number
  maxNodes: number
  maxArrayItems: number
  maxObjectProperties: number
}

interface ValidationState {
  nodes: number
  readonly options: BoundedJsonSchemaOptions
}

/**
 * Validates one JSON value against the portable JSON-Schema subset used by MCP tool inputs.
 * It deliberately has no coercion/default insertion and fails closed for malformed schemas.
 */
export function matchesBoundedJsonSchema(
  value: unknown,
  schema: unknown,
  options: BoundedJsonSchemaOptions
): boolean {
  return matches(value, schema, 0, { nodes: 0, options })
}

function matches(
  value: unknown,
  schemaValue: unknown,
  depth: number,
  state: ValidationState
): boolean {
  if (depth > state.options.maxDepth || ++state.nodes > state.options.maxNodes) return false
  if (schemaValue === true) return true
  if (schemaValue === false || !isRecord(schemaValue)) return false
  const schema = schemaValue

  if (
    Array.isArray(schema.allOf) &&
    !schema.allOf.every((child) => matches(value, child, depth + 1, state))
  ) {
    return false
  }
  if (
    Array.isArray(schema.anyOf) &&
    !schema.anyOf.some((child) => matches(value, child, depth + 1, state))
  ) {
    return false
  }
  if (Array.isArray(schema.oneOf)) {
    let count = 0
    for (const child of schema.oneOf) {
      if (matches(value, child, depth + 1, state)) count += 1
      if (count > 1) return false
    }
    if (count !== 1) return false
  }
  if (schema.not !== undefined && matches(value, schema.not, depth + 1, state)) return false
  if (schema.if !== undefined) {
    const conditionMatches = matches(value, schema.if, depth + 1, state)
    if (
      conditionMatches &&
      schema.then !== undefined &&
      !matches(value, schema.then, depth + 1, state)
    ) {
      return false
    }
    if (
      !conditionMatches &&
      schema.else !== undefined &&
      !matches(value, schema.else, depth + 1, state)
    ) {
      return false
    }
  }

  if (schema.const !== undefined && !jsonEquals(value, schema.const)) return false
  if (
    Array.isArray(schema.enum) &&
    !schema.enum.some((candidate) => jsonEquals(value, candidate))
  ) {
    return false
  }
  if (schema.nullable === true && value === null) return true

  const types = Array.isArray(schema.type) ? schema.type : [schema.type]
  if (schema.type !== undefined && !types.some((type) => matchesType(value, type))) return false

  if (Array.isArray(value)) return matchesArray(value, schema, depth, state)
  if (isRecord(value)) return matchesObject(value, schema, depth, state)
  if (typeof value === 'string') return matchesString(value, schema)
  if (typeof value === 'number') return matchesNumber(value, schema)
  return value === null || typeof value === 'boolean'
}

function matchesArray(
  value: unknown[],
  schema: Record<string, unknown>,
  depth: number,
  state: ValidationState
): boolean {
  if (value.length > state.options.maxArrayItems) return false
  if (typeof schema.minItems === 'number' && value.length < schema.minItems) return false
  if (typeof schema.maxItems === 'number' && value.length > schema.maxItems) return false
  if (schema.uniqueItems === true) {
    for (let index = 0; index < value.length; index += 1) {
      if (value.slice(0, index).some((candidate) => jsonEquals(candidate, value[index])))
        return false
    }
  }

  const prefixItems = Array.isArray(schema.prefixItems) ? schema.prefixItems : []
  for (let index = 0; index < Math.min(prefixItems.length, value.length); index += 1) {
    if (!matches(value[index], prefixItems[index], depth + 1, state)) return false
  }
  if (schema.items !== undefined) {
    const start = prefixItems.length
    for (let index = start; index < value.length; index += 1) {
      if (!matches(value[index], schema.items, depth + 1, state)) return false
    }
  } else if (prefixItems.length === 0 && value.length > 0) {
    // A typed array without an item schema is not portable enough for a managed Tool contract.
    return false
  }
  if (schema.contains !== undefined) {
    const count = value.filter((child) => matches(child, schema.contains, depth + 1, state)).length
    const minimum = typeof schema.minContains === 'number' ? schema.minContains : 1
    const maximum =
      typeof schema.maxContains === 'number' ? schema.maxContains : Number.MAX_SAFE_INTEGER
    if (count < minimum || count > maximum) return false
  }
  return true
}

function matchesObject(
  value: Record<string, unknown>,
  schema: Record<string, unknown>,
  depth: number,
  state: ValidationState
): boolean {
  const keys = Object.keys(value)
  if (keys.length > state.options.maxObjectProperties) return false
  if (typeof schema.minProperties === 'number' && keys.length < schema.minProperties) return false
  if (typeof schema.maxProperties === 'number' && keys.length > schema.maxProperties) return false
  if (
    Array.isArray(schema.required) &&
    schema.required.some(
      (key) => typeof key !== 'string' || !Object.prototype.hasOwnProperty.call(value, key)
    )
  ) {
    return false
  }
  if (schema.propertyNames !== undefined) {
    for (const key of keys) {
      if (!matches(key, schema.propertyNames, depth + 1, state)) return false
    }
  }

  const properties = isRecord(schema.properties) ? schema.properties : {}
  const patternProperties = isRecord(schema.patternProperties) ? schema.patternProperties : {}
  for (const [key, child] of Object.entries(value)) {
    let matched = false
    if (Object.prototype.hasOwnProperty.call(properties, key)) {
      matched = true
      if (!matches(child, properties[key], depth + 1, state)) return false
    }
    for (const [pattern, childSchema] of Object.entries(patternProperties)) {
      if (!safeRegExp(pattern)?.test(key)) continue
      matched = true
      if (!matches(child, childSchema, depth + 1, state)) return false
    }
    if (!matched) {
      if (schema.additionalProperties === false) return false
      if (
        isRecord(schema.additionalProperties) ||
        typeof schema.additionalProperties === 'boolean'
      ) {
        if (!matches(child, schema.additionalProperties, depth + 1, state)) return false
      }
    }
  }

  if (isRecord(schema.dependentRequired)) {
    for (const [key, dependencies] of Object.entries(schema.dependentRequired)) {
      if (!Object.prototype.hasOwnProperty.call(value, key)) continue
      if (
        !Array.isArray(dependencies) ||
        dependencies.some(
          (dependency) =>
            typeof dependency !== 'string' ||
            !Object.prototype.hasOwnProperty.call(value, dependency)
        )
      ) {
        return false
      }
    }
  }
  return true
}

function matchesString(value: string, schema: Record<string, unknown>): boolean {
  const length = [...value].length
  if (typeof schema.minLength === 'number' && length < schema.minLength) return false
  if (typeof schema.maxLength === 'number' && length > schema.maxLength) return false
  if (typeof schema.pattern === 'string' && !safeRegExp(schema.pattern)?.test(value)) return false
  return true
}

function matchesNumber(value: number, schema: Record<string, unknown>): boolean {
  if (!Number.isFinite(value)) return false
  if (typeof schema.minimum === 'number' && value < schema.minimum) return false
  if (typeof schema.maximum === 'number' && value > schema.maximum) return false
  if (typeof schema.exclusiveMinimum === 'number' && value <= schema.exclusiveMinimum) return false
  if (typeof schema.exclusiveMaximum === 'number' && value >= schema.exclusiveMaximum) return false
  if (
    typeof schema.multipleOf === 'number' &&
    (schema.multipleOf <= 0 ||
      Math.abs(value / schema.multipleOf - Math.round(value / schema.multipleOf)) > 1e-12)
  ) {
    return false
  }
  return schema.type !== 'integer' || Number.isSafeInteger(value)
}

function matchesType(value: unknown, type: unknown): boolean {
  switch (type) {
    case 'null':
      return value === null
    case 'object':
      return isRecord(value)
    case 'array':
      return Array.isArray(value)
    case 'string':
      return typeof value === 'string'
    case 'number':
      return typeof value === 'number' && Number.isFinite(value)
    case 'integer':
      return typeof value === 'number' && Number.isSafeInteger(value)
    case 'boolean':
      return typeof value === 'boolean'
    default:
      return false
  }
}

function safeRegExp(pattern: string): RegExp | undefined {
  try {
    return new RegExp(pattern, 'u')
  } catch {
    return undefined
  }
}

function jsonEquals(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) return true
  if (Array.isArray(left) && Array.isArray(right)) {
    return (
      left.length === right.length && left.every((value, index) => jsonEquals(value, right[index]))
    )
  }
  if (isRecord(left) && isRecord(right)) {
    const leftKeys = Object.keys(left).sort()
    const rightKeys = Object.keys(right).sort()
    return (
      leftKeys.length === rightKeys.length &&
      leftKeys.every((key, index) => key === rightKeys[index] && jsonEquals(left[key], right[key]))
    )
  }
  return false
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}
