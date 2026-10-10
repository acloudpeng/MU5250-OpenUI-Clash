// Reads a QR code from an image in the browser; the decoder (jsQR) loads only when used.
// Camera scanning is not offered: browsers allow it only on https:// pages.

/** Larger photos are scaled down first; a QR code stays readable well below this. */
const MAX_SIDE = 1600

async function pixels(file: Blob) {
  const bitmap = await createImageBitmap(file)
  const scale = Math.min(1, MAX_SIDE / Math.max(bitmap.width, bitmap.height))
  const width = Math.max(1, Math.round(bitmap.width * scale))
  const height = Math.max(1, Math.round(bitmap.height * scale))
  const canvas = document.createElement('canvas')
  canvas.width = width
  canvas.height = height
  const context = canvas.getContext('2d', { willReadFrequently: true })
  if (!context) throw new Error('canvas unavailable') // i18n-ignore
  context.drawImage(bitmap, 0, 0, width, height)
  bitmap.close()
  return { data: context.getImageData(0, 0, width, height).data, width, height }
}

/** The text of the first QR code in the image, or null when none is found. */
export async function readQr(file: Blob): Promise<string | null> {
  const [{ data, width, height }, { default: jsQR }] = await Promise.all([pixels(file), import('jsqr')])
  return jsQR(data, width, height, { inversionAttempts: 'attemptBoth' })?.data ?? null
}

export interface Activation {
  smdp: string
  matchingId: string
  /** The code asks for a confirmation code (its fifth field is 1). */
  needsConfirmation: boolean
}

const SMDP = /^[A-Za-z0-9][A-Za-z0-9.:-]{0,254}$/
const MATCHING_ID = /^[A-Za-z0-9-]{0,255}$/

/** `LPA:1$address$matchingId[$oid[$1]]`, checked as the agent checks it (agent/src/esim.rs). */
export function parseActivation(text: string): Activation | null {
  const code = text.trim()
  if (code.slice(0, 4).toUpperCase() !== 'LPA:') return null
  const parts = code.slice(4).split('$')
  if (parts[0] !== '1' || parts.length < 2 || parts.length > 5) return null
  const [, smdp, matchingId = '', , flag] = parts
  if (!SMDP.test(smdp) || !MATCHING_ID.test(matchingId)) return null
  return { smdp: smdp.toLowerCase(), matchingId, needsConfirmation: flag === '1' }
}

export const validSmdp = (s: string) => SMDP.test(s.trim())
export const validMatchingId = (s: string) => MATCHING_ID.test(s.trim())
