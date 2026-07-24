#!/usr/bin/env node
/* eslint-disable @typescript-eslint/explicit-function-return-type -- User-materialized JavaScript template; the managed runtime and post-generation checks define its contract. */
/** Patch one copy and rerun it with a static --output; the Host observes it automatically. */

import { existsSync } from 'node:fs'
import { randomUUID } from 'node:crypto'
import { mkdir, open, rename, rm, stat } from 'node:fs/promises'
import { basename, dirname, isAbsolute, join, relative, resolve } from 'node:path'
import pptxgen from 'pptxgenjs'

function values(flag) {
  const result = []
  for (let index = 0; index < process.argv.length; index += 1) {
    if (process.argv[index] === flag && process.argv[index + 1]) {
      result.push(process.argv[index + 1])
      index += 1
    }
  }
  return result
}

function value(flag, fallback) {
  return values(flag)[0] ?? fallback
}

function mountedInput(mountPath) {
  const rootValue = process.env.MYCOPILOT_INPUT_ROOT
  if (!rootValue) throw new Error('MYCOPILOT_INPUT_ROOT is required when --image is used')
  const root = resolve(rootValue)
  const candidate = resolve(root, mountPath)
  const child = relative(root, candidate)
  if (
    child === '..' ||
    child.startsWith(`..${process.platform === 'win32' ? '\\' : '/'}`) ||
    isAbsolute(child)
  ) {
    throw new Error(`input escapes MYCOPILOT_INPUT_ROOT: ${mountPath}`)
  }
  if (!existsSync(candidate)) throw new Error(`mounted input not found: ${mountPath}`)
  return candidate
}

function addFooter(slide, pageNumber) {
  slide.addText('MyCopilot', {
    x: 0.55,
    y: 7.05,
    w: 11.2,
    h: 0.2,
    fontFace: 'Microsoft YaHei',
    fontSize: 8,
    color: '94A3B8',
    margin: 0
  })
  slide.addText(String(pageNumber), {
    x: 12,
    y: 7.05,
    w: 0.7,
    h: 0.2,
    fontFace: 'Arial',
    fontSize: 8,
    color: '94A3B8',
    align: 'right',
    margin: 0
  })
}

function buildPresentation(title, images) {
  const deck = new pptxgen()
  deck.layout = 'LAYOUT_WIDE'
  deck.author = 'MyCopilot'
  deck.subject = title
  deck.title = title
  deck.company = 'MyCopilot'
  deck.lang = 'zh-CN'
  deck.theme = {
    headFontFace: 'Microsoft YaHei',
    bodyFontFace: 'Microsoft YaHei',
    lang: 'zh-CN'
  }

  // Patch this slide-building block; keep one builder and rerun it after every revision.
  const slide = deck.addSlide()
  slide.background = { color: '0F172A' }
  slide.addShape(deck.ShapeType.rect, {
    x: 0,
    y: 0,
    w: 0.18,
    h: 7.5,
    line: { color: '38BDF8', transparency: 100 },
    fill: { color: '38BDF8' }
  })
  slide.addText(title, {
    x: 0.85,
    y: 0.8,
    w: 11.6,
    h: 0.7,
    fontFace: 'Microsoft YaHei',
    fontSize: 28,
    bold: true,
    color: 'F8FAFC',
    margin: 0
  })
  slide.addText('Replace this text with the requested presentation narrative.', {
    x: 0.9,
    y: 1.75,
    w: images.length ? 5.2 : 11,
    h: 1.2,
    fontFace: 'Microsoft YaHei',
    fontSize: 18,
    color: 'CBD5E1',
    breakLine: false,
    margin: 0.04,
    valign: 'mid'
  })
  if (images[0]) {
    slide.addImage({ path: images[0], x: 6.45, y: 1.65, w: 5.8, h: 4.45 })
  }
  addFooter(slide, 1)
  return deck
}

async function main() {
  const output = resolve(value('--output', 'presentation.pptx'))
  if (!output.toLowerCase().endsWith('.pptx')) throw new Error('--output must end in .pptx')
  const title = value('--title', 'Presentation title')
  const images = values('--image').map(mountedInput)
  const outputDirectory = dirname(output)
  await mkdir(outputDirectory, { recursive: true })
  const temporary = join(outputDirectory, `.${basename(output)}.${randomUUID()}.pptx`)
  try {
    await buildPresentation(title, images).writeFile({ fileName: temporary })
    const metadata = await stat(temporary)
    const signature = Buffer.alloc(4)
    const file = await open(temporary, 'r')
    try {
      await file.read(signature, 0, signature.length, 0)
    } finally {
      await file.close()
    }
    if (
      !metadata.isFile() ||
      metadata.size < 4 ||
      signature[0] !== 0x50 ||
      signature[1] !== 0x4b ||
      signature[2] !== 0x03 ||
      signature[3] !== 0x04
    ) {
      throw new Error('generated presentation is not a non-empty OOXML ZIP package')
    }
    // Same-directory rename keeps publication atomic on the target filesystem.
    await rename(temporary, output)
  } finally {
    await rm(temporary, { force: true })
  }
  process.stdout.write(`${JSON.stringify({ status: 'created', output, images: images.length })}\n`)
}

await main()
