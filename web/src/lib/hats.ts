// The selected hat (frontend spec §5): server data, picked by the operator,
// remembered in this browser. `''` means every hat.
import type { Theme } from './theme'

export const HAT_KEY = 'hennery.hat'

export function readHat(): string {
  try {
    return localStorage.getItem(HAT_KEY) ?? ''
  } catch {
    return ''
  }
}

export function writeHat(hat: string): void {
  try {
    localStorage.setItem(HAT_KEY, hat)
  } catch {
    // No storage: the hat holds for this load only.
  }
}

const COLOUR = /^#[0-9a-fA-F]{6}$/

/** `colour` darkened, for the gradient's second stop (the default pair is
 *  about this far apart). */
function shade(colour: string): string {
  const n = parseInt(colour.slice(1), 16)
  const channel = (shift: number) => Math.round(((n >> shift) & 0xff) * 0.77)
  return '#' + [16, 8, 0].map((shift) => channel(shift).toString(16).padStart(2, '0')).join('')
}

/** The theme a hat's colour gives, or `null` (the default colours) when it
 *  is not `#rrggbb`. */
export function themeOf(colour: string | undefined): Theme | null {
  if (colour === undefined || !COLOUR.test(colour)) return null
  const accent = colour.toLowerCase()
  return { accent, accent2: shade(accent) }
}
