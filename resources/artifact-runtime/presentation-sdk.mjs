/* eslint-disable @typescript-eslint/explicit-function-return-type -- This runtime SDK publishes JavaScript contracts to managed editor scripts. */

import { writeFile } from 'node:fs/promises'

const PLAN_ENV = 'MYCOPILOT_PRESENTATION_EDIT_PLAN'
const INPUT_PLACEHOLDER_PREFIX = '__mycopilot_agent_input__/'
const MAX_OPERATIONS = 256
let submitted = false

function record(type, value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`${type} requires one options object`)
  }
  return Object.freeze({ type, ...value })
}

function requiredString(value, label) {
  if (typeof value !== 'string' || value.length === 0 || value.trim() !== value) {
    throw new TypeError(`${label} must be a non-empty, trimmed string`)
  }
  if (value.includes('\0') || value.includes('\n') || value.includes('\r')) {
    throw new TypeError(`${label} contains an unsupported control character`)
  }
  return value
}

function requiredPositiveInteger(value, label) {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new TypeError(`${label} must be a positive integer`)
  }
  return value
}

function requiredNonNegativeInteger(value, label) {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new TypeError(`${label} must be a non-negative integer`)
  }
  return value
}

function inputReference(value, label = 'input') {
  if (!value || value.type !== 'input' || typeof value.mountPath !== 'string') {
    throw new TypeError(`${label} must be created by input()`)
  }
  return value
}

function outputReference(value) {
  if (!value || value.type !== 'output' || typeof value.path !== 'string') {
    throw new TypeError('destination must be created by output()')
  }
  return value
}

function position(value) {
  if (value == null) return undefined
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError('position must be an object')
  }
  if (value.type === 'index') {
    return { type: 'index', index: requiredNonNegativeInteger(value.index, 'position.index') }
  }
  if (value.type !== 'after' && value.type !== 'before') {
    throw new TypeError('position.type must be index, after, or before')
  }
  return {
    type: value.type,
    target: requiredString(value.target, 'position.target')
  }
}

function pushOperation(operations, operation) {
  if (operations.length >= MAX_OPERATIONS) {
    throw new RangeError(`presentation edit plan cannot exceed ${MAX_OPERATIONS} operations`)
  }
  operations.push(operation)
}

function slideTarget(slideNumber) {
  return `/slide[${requiredPositiveInteger(slideNumber, 'slideNumber')}]`
}

function isWholeSlideOperation(operation) {
  return (
    (operation.type === 'add' && operation.parent === '/' && operation.elementType === 'slide') ||
    (operation.type === 'remove' && /^\/slide\[[1-9][0-9]*\]$/.test(operation.target)) ||
    (operation.type === 'move' && /^\/slide\[[1-9][0-9]*\]$/.test(operation.target))
  )
}

function validateWholeSlideOrdering(operations) {
  const structural = operations
    .map((operation, index) => ({ operation, index }))
    .filter(({ operation }) => isWholeSlideOperation(operation))
  if (
    structural.length > 1 ||
    (structural.length === 1 && structural[0].index !== operations.length - 1)
  ) {
    throw new Error(
      'a presentation edit may contain at most one whole-slide operation and it must be last'
    )
  }
}

function facade(operations) {
  return Object.freeze({
    addSlide({ layout = null, title = null, body = null, backgroundColor = null } = {}) {
      const properties = {}
      if (layout != null) properties.layout = requiredString(layout, 'addSlide.layout')
      if (title != null) properties.title = requiredString(title, 'addSlide.title')
      if (body != null) properties.text = String(body)
      if (backgroundColor != null) {
        properties.background = requiredString(backgroundColor, 'addSlide.backgroundColor')
      }
      pushOperation(
        operations,
        record('add', {
          parent: '/',
          elementType: 'slide',
          copyFrom: null,
          position: null,
          properties,
          force: false
        })
      )
    },

    removeSlide({ slideNumber }) {
      pushOperation(
        operations,
        record('remove', {
          target: slideTarget(slideNumber),
          shift: null,
          properties: {}
        })
      )
    },

    moveSlide({ slideNumber, newIndex }) {
      pushOperation(
        operations,
        record('move', {
          target: slideTarget(slideNumber),
          newParent: null,
          position: {
            type: 'index',
            index: requiredPositiveInteger(newIndex, 'moveSlide.newIndex') - 1
          },
          properties: {}
        })
      )
    },

    set({ target, properties = {} }) {
      pushOperation(
        operations,
        record('set', {
          target: requiredString(target, 'set.target'),
          properties,
          replacement: null,
          force: false
        })
      )
    },

    replaceText({ target, find, replace }) {
      pushOperation(
        operations,
        record('set', {
          target: requiredString(target, 'replaceText.target'),
          properties: {},
          replacement: {
            find: requiredString(find, 'replaceText.find'),
            replace: String(replace ?? '')
          },
          force: false
        })
      )
    },

    add({
      parent,
      elementType,
      copyFrom = null,
      position: requestedPosition = null,
      properties = {}
    }) {
      pushOperation(
        operations,
        record('add', {
          parent: requiredString(parent, 'add.parent'),
          elementType: requiredString(elementType, 'add.elementType'),
          copyFrom: copyFrom == null ? null : requiredString(copyFrom, 'add.copyFrom'),
          position: position(requestedPosition) ?? null,
          properties,
          force: false
        })
      )
    },

    remove({ target, properties = {} }) {
      pushOperation(
        operations,
        record('remove', {
          target: requiredString(target, 'remove.target'),
          shift: null,
          properties
        })
      )
    },

    move({ target, newParent = null, position: requestedPosition = null, properties = {} }) {
      pushOperation(
        operations,
        record('move', {
          target: requiredString(target, 'move.target'),
          newParent: newParent == null ? null : requiredString(newParent, 'move.newParent'),
          position: position(requestedPosition) ?? null,
          properties
        })
      )
    },

    swap({ firstTarget, secondTarget }) {
      pushOperation(
        operations,
        record('swap', {
          firstTarget: requiredString(firstTarget, 'swap.firstTarget'),
          secondTarget: requiredString(secondTarget, 'swap.secondTarget')
        })
      )
    },

    replaceImage({ target, source }) {
      const image = inputReference(source, 'replaceImage.source')
      pushOperation(
        operations,
        record('set', {
          target: requiredString(target, 'replaceImage.target'),
          properties: {
            src: { resourcePath: `${INPUT_PLACEHOLDER_PREFIX}${image.mountPath}` }
          },
          replacement: null,
          force: false
        })
      )
    },

    updateTableCell({ target, text }) {
      pushOperation(
        operations,
        record('set', {
          target: requiredString(target, 'updateTableCell.target'),
          properties: { text: String(text ?? '') },
          replacement: null,
          force: false
        })
      )
    },

    updateChart({ target, properties }) {
      if (!properties || typeof properties !== 'object' || Array.isArray(properties)) {
        throw new TypeError('updateChart.properties must be an object')
      }
      const { categories, series } = properties
      if (!Array.isArray(categories) || !Array.isArray(series)) {
        throw new TypeError(
          'updateChart properties.categories and properties.series must be arrays'
        )
      }
      if (categories.length === 0 || series.length === 0 || series.length > 32) {
        throw new RangeError('updateChart requires categories and between 1 and 32 series')
      }
      const encodedCategories = categories.map((value, index) => {
        const category = String(value)
        if (
          category.length === 0 ||
          category.includes(',') ||
          category.includes('\n') ||
          category.includes('\r')
        ) {
          throw new TypeError(
            `updateChart categories[${index}] cannot be empty or contain commas/newlines`
          )
        }
        return category
      })
      const chartProperties = {
        categories: encodedCategories.join(',')
      }
      series.forEach((item, index) => {
        if (!item || typeof item !== 'object' || !Array.isArray(item.values)) {
          throw new TypeError(`updateChart series[${index}] must have name and values`)
        }
        const name = requiredString(item.name, `series[${index}].name`)
        if (name.includes(':') || name.includes(',')) {
          throw new TypeError(`updateChart series[${index}].name cannot contain colon or comma`)
        }
        if (item.values.length !== encodedCategories.length) {
          throw new RangeError(`updateChart series[${index}] values must match categories length`)
        }
        const values = item.values.map((value, valueIndex) => {
          if (typeof value !== 'number' || !Number.isFinite(value)) {
            throw new TypeError(
              `updateChart series[${index}].values[${valueIndex}] must be finite number`
            )
          }
          return String(value)
        })
        chartProperties[`series${index + 1}`] = `${name}:${values.join(',')}`
      })
      pushOperation(
        operations,
        record('set', {
          target: requiredString(target, 'updateChart.target'),
          properties: chartProperties,
          replacement: null,
          force: false
        })
      )
    }
  })
}

export function input(mountPath) {
  return Object.freeze({ type: 'input', mountPath: requiredString(mountPath, 'input mountPath') })
}

export function output(path) {
  return Object.freeze({ type: 'output', path: requiredString(path, 'output path') })
}

export async function editPresentation({ source, destination, mode = 'saveAs', edit }) {
  if (submitted) throw new Error('editPresentation() may be called only once')
  submitted = true
  const sourceRef = inputReference(source, 'source')
  const destinationRef = outputReference(destination)
  if (mode !== 'saveAs') throw new Error('Presentation Editor supports only mode="saveAs"')
  if (typeof edit !== 'function') throw new TypeError('edit must be a function')

  const operations = []
  await edit(facade(operations))
  if (operations.length === 0) throw new Error('presentation edit plan contains no operations')
  validateWholeSlideOrdering(operations)

  const planPath = process.env[PLAN_ENV]
  if (!planPath) throw new Error(`${PLAN_ENV} is missing from the managed Host runtime`)
  const plan = {
    schemaVersion: 1,
    source: sourceRef,
    destination: destinationRef,
    mode,
    operations
  }
  await writeFile(planPath, `${JSON.stringify(plan)}\n`, {
    encoding: 'utf8',
    flag: 'wx',
    mode: 0o600
  })
}
