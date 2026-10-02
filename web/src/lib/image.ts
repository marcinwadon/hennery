export const MAX_IMAGE_BYTES = 5 * 1024 * 1024 // 5 MiB, matches the collector cap
const ALLOWED = new Set(['image/png', 'image/jpeg', 'image/gif', 'image/webp'])

/** Returns an error string if the file isn't an allowed image, else null. */
export function isAllowedImage(file: File): string | null {
  if (!ALLOWED.has(file.type)) return `unsupported image type: ${file.type || 'unknown'}`
  if (file.size > MAX_IMAGE_BYTES) return `image too large (max 5 MiB): ${file.name}`
  return null
}

/** Reads a File as base64 with the `data:...;base64,` prefix stripped. */
export function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const fr = new FileReader()
    fr.onerror = () => reject(fr.error)
    fr.onload = () => {
      const res = String(fr.result)
      const comma = res.indexOf(',')
      resolve(comma >= 0 ? res.slice(comma + 1) : res)
    }
    fr.readAsDataURL(file)
  })
}
