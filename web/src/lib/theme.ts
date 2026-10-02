// The colours `/theme.js` applies before the first paint (frontend spec §8).
// The app saves them when the selected hat changes (4c, 4d), from the hat's
// colours on the server, and applies them at once.

export const THEME_KEY = 'hennery.theme'

export interface Theme {
  accent: string
  accent2: string
}

const COLOUR = /^#[0-9a-fA-F]{6}$/

/** Save `theme` for the next load and apply it now. A colour that is not
 *  `#rrggbb` is refused, as `/theme.js` would refuse it. */
export function saveTheme(theme: Theme | null): void {
  const root = document.documentElement.style
  if (theme === null) {
    root.removeProperty('--accent')
    root.removeProperty('--accent-2')
    try {
      localStorage.removeItem(THEME_KEY)
    } catch {
      // No storage: the default colours stay anyway.
    }
    return
  }
  if (!COLOUR.test(theme.accent) || !COLOUR.test(theme.accent2)) throw new Error('A theme colour must be #rrggbb.')
  root.setProperty('--accent', theme.accent)
  root.setProperty('--accent-2', theme.accent2)
  try {
    localStorage.setItem(THEME_KEY, JSON.stringify({ '--accent': theme.accent, '--accent-2': theme.accent2 }))
  } catch {
    // No storage: applied for this load only.
  }
}
