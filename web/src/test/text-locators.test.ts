// The loose-text-locator guard: Playwright's `getByText('x')` is a
// case-insensitive SUBSTRING match by default, so `getByText('Saved.')` also
// matched "...rules as saved." and turned main red only when that other text
// happened to be on screen (#109 -> #110). Testing Library's matchers are
// exact by default, so `web/src` tests get only the narrower checks.
//
// The scanners are in `./textLocators.ts`. The snippet tests below give
// each verdict its own case; the last block runs the scanners over the
// real `web/e2e` and `web/src` trees, which is what makes `pnpm test` fail
// on a new loose locator.
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { describe, expect, it } from 'vitest'
import { scanPlaywrightLocators, scanTestingLibraryLocators } from './textLocators'

// Vitest runs from `web/`.
const ROOT = process.cwd()

function filesUnder(dir: string, test: RegExp): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) return filesUnder(path, test)
    return test.test(name) ? [path] : []
  })
}

describe('scanPlaywrightLocators (web/e2e/**/*.ts)', () => {
  it('flags a string literal with no { exact: true }', () => {
    const v = scanPlaywrightLocators('f.ts', "await expect(page.getByText('Saved.')).toBeVisible()")
    expect(v).toHaveLength(1)
    expect(v[0].line).toBe(1)
  })

  it('passes a string literal with { exact: true }', () => {
    expect(scanPlaywrightLocators('f.ts', "await expect(page.getByText('Saved.', { exact: true })).toBeVisible()")).toEqual([])
  })

  it('flags an unanchored regex literal', () => {
    const v = scanPlaywrightLocators('f.ts', "await page.getByLabel(/Password at least/).fill(x)")
    expect(v).toHaveLength(1)
  })

  it('passes a regex literal anchored with ^', () => {
    expect(scanPlaywrightLocators('f.ts', "await page.getByLabel(/^Password at least/).fill(x)")).toEqual([])
  })

  it('passes a regex literal anchored with $', () => {
    expect(scanPlaywrightLocators('f.ts', 'await expect(page.getByText(/Paired: e2e host$/)).toBeVisible()')).toEqual([])
  })

  it('flags a regex literal whose only $ is escaped (a dollar sign, not the end)', () => {
    expect(scanPlaywrightLocators('f.ts', String.raw`await page.getByText(/costs \$/).click()`)).toHaveLength(1)
  })

  it('passes a regex literal ending in an escaped backslash and then $ (the end)', () => {
    expect(scanPlaywrightLocators('f.ts', String.raw`await page.getByText(/C:\\$/).click()`)).toEqual([])
  })

  it('flags a template literal with no { exact: true }, plain or with a ${} hole', () => {
    expect(scanPlaywrightLocators('f.ts', 'await page.getByText(`Saved.`).click()')).toHaveLength(1)
    expect(scanPlaywrightLocators('f.ts', 'await page.getByLabel(`Hat ${n}`).click()')).toHaveLength(1)
  })

  it('passes a template literal with { exact: true }', () => {
    expect(scanPlaywrightLocators('f.ts', 'await page.getByLabel(`Hat ${n}`, { exact: true }).click()')).toEqual([])
  })

  it('does not judge a locator built from a variable', () => {
    expect(scanPlaywrightLocators('f.ts', 'await page.getByText(label).click()')).toEqual([])
  })
})

describe('scanTestingLibraryLocators (web/src/**/*.test.ts(x))', () => {
  it('flags an explicit { exact: false }', () => {
    const v = scanTestingLibraryLocators('f.tsx', "screen.getByText('This turn holds more than is shown here', { exact: false })")
    expect(v).toHaveLength(1)
  })

  it('passes a plain string literal (Testing Library matches exactly by default)', () => {
    expect(scanTestingLibraryLocators('f.tsx', "screen.getByText('Saved.')")).toEqual([])
  })

  it('flags a short, unanchored regex literal', () => {
    const v = scanTestingLibraryLocators('f.tsx', 'screen.getByText(/cut or left out/)')
    expect(v).toHaveLength(1)
  })

  it('passes a short regex literal anchored with ^', () => {
    expect(scanTestingLibraryLocators('f.tsx', 'screen.getByLabelText(/^Public URL/)')).toEqual([])
  })

  it('passes a short regex literal anchored with $', () => {
    expect(scanTestingLibraryLocators('f.tsx', 'screen.getByText(/e2e host$/)')).toEqual([])
  })

  it('flags a short regex literal whose only $ is escaped', () => {
    expect(scanTestingLibraryLocators('f.tsx', String.raw`screen.getByText(/costs \$/)`)).toHaveLength(1)
  })

  it('passes a long unanchored regex literal (20 chars of pattern or more)', () => {
    expect(scanTestingLibraryLocators('f.tsx', 'screen.getByText(/This field cannot be filled in here/)')).toEqual([])
    // Exactly 20 characters of pattern is long enough.
    expect(scanTestingLibraryLocators('f.tsx', 'screen.getByText(/this field is filled/)')).toEqual([])
  })
})

describe('the web test sources', () => {
  // This file's own snippets above are deliberately-violating fixtures (data,
  // not real locators): it is the guard's test, not something it guards.
  const SELF = join(ROOT, 'src', 'test', 'text-locators.test.ts')
  const e2eFiles = filesUnder(join(ROOT, 'e2e'), /\.ts$/)
  const srcTestFiles = filesUnder(join(ROOT, 'src'), /\.test\.tsx?$/).filter((f) => f !== SELF)

  it('are all read', () => {
    expect(e2eFiles.length).toBeGreaterThan(4)
    expect(srcTestFiles.length).toBeGreaterThan(30)
  })

  it('use exact or anchored text locators in web/e2e', () => {
    const violations = e2eFiles.flatMap((f) => scanPlaywrightLocators(relative(ROOT, f), readFileSync(f, 'utf8')))
    expect(violations.map((v) => `${v.file}:${v.line}: ${v.snippet}`)).toEqual([])
  })

  it('never opt into substring text matching in web/src tests', () => {
    const violations = srcTestFiles.flatMap((f) => scanTestingLibraryLocators(relative(ROOT, f), readFileSync(f, 'utf8')))
    expect(violations.map((v) => `${v.file}:${v.line}: ${v.snippet}`)).toEqual([])
  })
})
