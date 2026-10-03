// A cheap guard against loose text locators in the web tests.
//
// The lesson it keeps: Playwright's `page.getByText('Saved.')` is a
// case-insensitive SUBSTRING match by default. It also matched "...rules as
// saved." elsewhere on the page, and turned main red only when that other
// text happened to be on screen (#109 -> #110). A text locator is therefore
// a full string with `{ exact: true }`, an anchored regex, or a role.
//
// These are pure string scanners (no filesystem access), so each verdict
// has its own snippet test; `text-locators.test.ts`, beside this file,
// also points them at the real `web/e2e` and `web/src` trees.
//
// What they assume (regex and text scanning over type-checked TypeScript,
// not a JS parser):
//   - brackets are balanced outside strings, and a template literal's
//     `${...}` holds no unbalanced bracket and no nested template literal;
//   - a bare `/` outside a string always starts a regex literal: true for
//     call arguments here, which never divide numbers;
//   - only the first argument is judged, and only the other arguments' raw
//     text is searched for `exact: true` / `exact: false`.
//
// Known limits (they let a loose locator through; none is a false alarm):
//   - A first argument that is not a bare literal is not judged: a variable
//     (`getByText(text)`), a matcher function, a wrapped or cast literal
//     (`('x')`, `'x' as const`).
//   - A `//` comment holding an apostrophe or a quote inside a call's
//     arguments makes the scanner read on as if in a string. It then throws
//     (unbalanced; the test fails loudly) or splits the arguments wrongly,
//     with no message.
//   - Playwright's `getByRole(role, { name: '<string>' })` is ALSO a
//     case-insensitive substring match unless `exact: true` is passed, and
//     it is NOT scanned yet (25 such calls in web/e2e when this was
//     written, some loose, e.g. `{ name: 'Create' }`). Nor are
//     `filter({ hasText: '...' })`, `getByAltText`, or `locator('text=...')`.
//     Scanning them is a follow-up.

export interface Violation {
  file: string
  line: number
  snippet: string
}

function lineOf(text: string, index: number): number {
  let line = 1
  for (let i = 0; i < index; i++) if (text[i] === '\n') line++
  return line
}

// Splits the arguments of the call whose `(` is at `openParenIdx` into their
// raw (untrimmed-of-inner-content, trimmed-of-surrounding-space) top-level
// source text, skipping over nested brackets, quoted strings and regex
// literals so a comma or bracket inside one of those never splits early.
function splitTopLevelArgs(text: string, openParenIdx: number): { args: string[]; endIdx: number } {
  let i = openParenIdx + 1
  let depth = 0
  let start = i
  const args: string[] = []
  while (i < text.length) {
    const c = text[i]
    if (c === "'" || c === '"' || c === '`') {
      const quote = c
      i++
      while (i < text.length && text[i] !== quote) {
        if (text[i] === '\\') i++
        i++
      }
      i++
      continue
    }
    if (c === '/') {
      // See the file-level comment: treated as a regex literal start.
      i++
      let inClass = false
      while (i < text.length && (inClass || text[i] !== '/')) {
        if (text[i] === '\\') i++
        else if (text[i] === '[') inClass = true
        else if (text[i] === ']') inClass = false
        i++
      }
      i++ // closing '/'
      while (i < text.length && /[a-z]/i.test(text[i])) i++ // flags
      continue
    }
    if (c === '(' || c === '{' || c === '[') {
      depth++
      i++
      continue
    }
    if (c === ')' && depth === 0) {
      args.push(text.slice(start, i).trim())
      return { args, endIdx: i + 1 }
    }
    if (c === ')' || c === '}' || c === ']') {
      depth--
      i++
      continue
    }
    if (c === ',' && depth === 0) {
      args.push(text.slice(start, i).trim())
      start = i + 1
      i++
      continue
    }
    i++
  }
  throw new Error(`textLocators: unbalanced parens scanning a call at index ${openParenIdx} of ${JSON.stringify(text.slice(openParenIdx, openParenIdx + 40))}`)
}

// A quoted string or a template literal (with or without `${...}`): to
// Playwright all three are a string, matched as a substring.
const STRING_LITERAL = /^'(?:[^'\\]|\\.)*'$|^"(?:[^"\\]|\\.)*"$|^`(?:[^`\\]|\\.)*`$/
// Captures the pattern body of a /.../flags regex literal.
const REGEX_LITERAL = /^\/((?:[^/\\[\]]|\\.|\[(?:[^\]\\]|\\.)*\])*)\/[a-z]*$/

function isStringLiteral(arg: string): boolean {
  return STRING_LITERAL.test(arg)
}

function regexBody(arg: string): string | null {
  const m = REGEX_LITERAL.exec(arg)
  return m ? m[1] : null
}

/** Anchored at its start (`^`) or at its end: a `$` after an even number
 *  of backslashes. In `/costs \$/` the `$` is a dollar sign, matched
 *  anywhere; in `/C:\\$/` it is the end. */
function isAnchored(body: string): boolean {
  if (body.startsWith('^')) return true
  if (!body.endsWith('$')) return false
  let slashes = 0
  for (let i = body.length - 2; i >= 0 && body[i] === '\\'; i--) slashes++
  return slashes % 2 === 0
}

function hasExact(rest: string, value: 'true' | 'false'): boolean {
  return new RegExp(`exact\\s*:\\s*${value}\\b`).test(rest)
}

function scanCalls(file: string, source: string, callRe: RegExp, judge: (arg0: string, rest: string) => boolean): Violation[] {
  const out: Violation[] = []
  let m: RegExpExecArray | null
  while ((m = callRe.exec(source))) {
    const openParen = m.index + m[0].length - 1
    const { args, endIdx } = splitTopLevelArgs(source, openParen)
    const arg0 = args[0] ?? ''
    const rest = args.slice(1).join(', ')
    if (judge(arg0, rest)) {
      out.push({ file, line: lineOf(source, m.index), snippet: source.slice(m.index, Math.min(endIdx, m.index + 160)) })
    }
    callRe.lastIndex = endIdx
  }
  return out
}

// web/e2e/**/*.ts: Playwright's `getByText`/`getByLabel`/`getByTitle`/
// `getByPlaceholder` do a case-insensitive SUBSTRING match on a string by
// default. A string literal must carry `{ exact: true }`; a regex literal
// must be anchored with `^` or `$` (otherwise it is just as loose a
// substring match).
export function scanPlaywrightLocators(file: string, source: string): Violation[] {
  const callRe = /\b(?:getByText|getByLabel|getByTitle|getByPlaceholder)\(/g
  return scanCalls(file, source, callRe, (arg0, rest) => {
    if (isStringLiteral(arg0)) return !hasExact(rest, 'true')
    const body = regexBody(arg0)
    if (body === null) return false // not a literal (e.g. a variable): not ours to judge
    return !isAnchored(body)
  })
}

// web/src/**/*.test.ts(x): Testing Library's `*ByText`/`*ByLabelText`/
// `*ByTitle` matchers are exact-string matches by default, so a string
// literal is already safe. Only flag an explicit `{ exact: false }` (opts
// back into substring matching), or a regex literal that is both short
// (under 20 characters of pattern - long enough to rarely collide) and
// unanchored.
export function scanTestingLibraryLocators(file: string, source: string): Violation[] {
  const callRe = /\b(?:get|query|find|getAll|queryAll|findAll)By(?:Text|LabelText|Title)\(/g
  return scanCalls(file, source, callRe, (arg0, rest) => {
    if (hasExact(rest, 'false')) return true
    const body = regexBody(arg0)
    if (body === null) return false
    return body.length < 20 && !isAnchored(body)
  })
}
