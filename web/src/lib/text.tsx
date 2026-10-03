// Server text shown as text (frontend spec §6.4, §8): names, labels, user
// agents and paths come from hosts, browsers and the operator, and may hold
// characters that are invisible or that reorder what follows them. Each
// such character is shown as a visible escape instead, and the result sits
// in a <bdi> so it cannot reorder the text around it (plan 3a: the
// collector refuses only a hand-picked set of format characters in host
// names; the whole category is escaped here).

/** Format (Cf: bidi controls, zero-width characters, the soft hyphen, the
 *  BOM, tag characters), control (Cc), line and paragraph separators (Zl,
 *  Zp), and every other character Unicode says renders as nothing
 *  (Default_Ignorable_Code_Point: the Hangul fillers, the variation
 *  selectors, the combining grapheme joiner). Combining marks (Mn) and
 *  private-use characters stay: scripts need the first, and the second
 *  render as a visible glyph. */
export const HIDDEN = /[\p{Cf}\p{Cc}\p{Zl}\p{Zp}\p{Default_Ignorable_Code_Point}]/u
const ALL_HIDDEN = new RegExp(HIDDEN.source, 'gu')

/** `text` with every hidden character written as `<U+XXXX>`. */
export function visible(text: string): string {
  return text.replace(ALL_HIDDEN, (c) => `<U+${c.codePointAt(0)!.toString(16).toUpperCase().padStart(4, '0')}>`)
}

/** Server text, escaped and isolated: the one way 4d renders it. */
export function Text({ children, className }: { children: string; className?: string }) {
  return <bdi className={className}>{visible(children)}</bdi>
}
