// The 44 px rule under 768 px (frontend spec §10), checked in jsdom: it does
// no layout and applies no media query, so this reads `manage.css` itself
// and asks which of its narrow-screen rules match an element.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

/** Puts `manage.css` in the document (Vitest loads no CSS); returns a
 *  remover. */
export function loadManageCss(): () => void {
  const style = document.createElement('style')
  style.textContent = readFileSync(join(process.cwd(), 'src/manage.css'), 'utf8')
  document.head.append(style)
  return () => style.remove()
}

/** Whether a narrow-screen rule gives `el` a `min-height` of 44 px or
 *  more. */
export function tallEnough(el: Element): boolean {
  for (const sheet of document.styleSheets) {
    for (const rule of sheet.cssRules) {
      if (!(rule instanceof CSSMediaRule) || !/max-width:\s*767px/.test(rule.media.mediaText)) continue
      for (const inner of rule.cssRules) {
        if (!(inner instanceof CSSStyleRule) || !el.matches(inner.selectorText)) continue
        if (parseFloat(inner.style.minHeight) >= 44) return true
      }
    }
  }
  return false
}

/** The controls in `root` too short to tap under 768 px, named. */
export function shortTargets(root: Element): string[] {
  return [...root.querySelectorAll('button, select, input[type="color"]')]
    .filter((el) => !tallEnough(el))
    .map((el) => el.getAttribute('aria-label') ?? (el.textContent?.trim() || el.closest('label')?.textContent || el.tagName))
}
