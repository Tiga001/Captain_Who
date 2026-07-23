/* eslint-disable @typescript-eslint/explicit-function-return-type -- electron-builder loads this JavaScript module directly. */

import {
  afterPack as verifyOfficeRendererAfterPack,
  afterSign as verifyOfficeRendererAfterSign
} from './verify-packaged-office-renderer.mjs'
import { verifyPackagedMacSignatures } from './verify-packaged-macos-signatures.mjs'

export async function afterPack(context) {
  await verifyOfficeRendererAfterPack(context)
}

export async function afterSign(context) {
  await verifyOfficeRendererAfterSign(context)
  if (context.electronPlatformName === 'darwin') {
    await verifyPackagedMacSignatures(context)
  }
}
