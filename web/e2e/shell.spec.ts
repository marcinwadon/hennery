// The web shell in a real browser, against the real binary (plan 4b): the
// setup link, a passkey registered at setup and used to sign in, the
// password, a step-up, the layout at 1280 px and 390 px by computed style,
// links into the app, and not one Content-Security-Policy violation.
import { expect, test, type CDPSession, type Page } from '@playwright/test'
import { startCollector, type Collector } from './collector'

const PASSWORD = 'correct horse battery staple'

test.describe.configure({ mode: 'serial' })

let collector: Collector
let page: Page
let cdp: CDPSession
const violations: string[] = []

test.beforeAll(async ({ browser }) => {
  collector = await startCollector()
  const context = await browser.newContext({ baseURL: collector.origin })
  page = await context.newPage()
  // Every CSP violation the page sees, whatever its source.
  await page.addInitScript(() => {
    document.addEventListener('securitypolicyviolation', (e) => {
      const seen = ((window as unknown as { __csp?: string[] }).__csp ??= [])
      seen.push(`${e.violatedDirective} ${e.blockedURI}`)
    })
  })
  page.on('console', (m) => {
    if (m.text().includes('Content Security Policy')) violations.push(m.text())
  })
  // A platform authenticator that verifies its user (Chromium's own).
  cdp = await context.newCDPSession(page)
  await cdp.send('WebAuthn.enable')
  await cdp.send('WebAuthn.addVirtualAuthenticator', {
    options: {
      protocol: 'ctap2',
      transport: 'internal',
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
    },
  })
})

test.afterAll(async () => {
  try {
    await page?.context().close()
  } finally {
    await collector?.stop()
  }
})

async function cspViolations(): Promise<string[]> {
  const seen = await page.evaluate(() => (window as unknown as { __csp?: string[] }).__csp ?? [])
  return [...violations, ...seen]
}

test('the setup link sets hennery up, its token gone from the address bar', async () => {
  const entries = await page.evaluate(() => history.length)
  await page.goto(collector.setupLink)
  await expect(page).toHaveURL(`${collector.origin}/setup`)
  expect(await page.evaluate(() => location.hash)).toBe('')
  // Replaced, not pushed: the visit adds one history entry, without the token.
  expect(await page.evaluate(() => history.length)).toBe(entries + 1)
  await expect(page.getByLabel(/^Public URL/)).toHaveValue(collector.origin)
  await page.getByLabel(/^Password at least/).fill(PASSWORD)
  await page.getByLabel('Password again', { exact: true }).fill(PASSWORD)
  await page.getByRole('button', { name: 'Set up' }).click()
  await expect(page.getByRole('heading', { name: 'hennery is set up' })).toBeVisible()
})

test('a passkey is registered right after setup, with no second password', async () => {
  await page.getByLabel('Passkey name', { exact: true }).fill('Test authenticator')
  await page.getByRole('button', { name: 'Add a passkey' }).click()
  await expect(page.getByText(/^Your passkey is added/)).toBeVisible()
  await page.getByRole('button', { name: 'Continue' }).click()
  await expect(page).toHaveURL(`${collector.origin}/sessions`)
  await expect(page.getByRole('heading', { name: 'Sessions' })).toBeVisible()
})

test('the shell: a rail from 1280 px, a bottom tab bar at 390 px', async () => {
  const display = (selector: string) =>
    page.locator(selector).evaluate((el) => getComputedStyle(el).display)
  await page.setViewportSize({ width: 1280, height: 800 })
  expect(await display('.rail')).toBe('flex')
  expect(await display('.tabbar')).toBe('none')
  expect(await display('.mtopbar')).toBe('none')
  await page.setViewportSize({ width: 390, height: 844 })
  expect(await display('.rail')).toBe('none')
  expect(await display('.tabbar')).toBe('flex')
  expect(await display('.mtopbar')).toBe('flex')
  const tab = page.getByRole('navigation', { name: 'Tabs' }).getByRole('link', { name: 'Hosts' })
  await expect(tab).toBeVisible()
  const box = await tab.boundingBox()
  expect(box!.height).toBeGreaterThanOrEqual(44)
  await tab.click()
  await expect(page).toHaveURL(`${collector.origin}/hosts`)
  await page.setViewportSize({ width: 1280, height: 800 })
})

test('a link into the app loads it, signed in', async () => {
  await page.goto('/hats')
  await expect(page.getByRole('heading', { name: 'Hats' })).toBeVisible()
})

test('signed out, a link goes to sign-in and back; the passkey signs in', async () => {
  await page.getByRole('button', { name: 'Sign out' }).click()
  await expect(page).toHaveURL(`${collector.origin}/login`)
  await page.goto('/hosts?x=1')
  await expect(page).toHaveURL(`${collector.origin}/login?next=%2Fhosts%3Fx%3D1`)
  await page.getByRole('button', { name: 'Sign in with passkey' }).click()
  await expect(page).toHaveURL(`${collector.origin}/hosts?x=1`)
  await expect(page.getByRole('heading', { name: 'Hosts' })).toBeVisible()
})

test('the password signs in, and a step-up from the page is accepted', async () => {
  await page.getByRole('button', { name: 'Sign out' }).click()
  await expect(page).toHaveURL(`${collector.origin}/login`)
  await page.getByLabel('Password', { exact: true }).fill(PASSWORD)
  await page.getByRole('button', { name: 'Sign in', exact: true }).click()
  await expect(page).toHaveURL(`${collector.origin}/sessions`)
  // What the step-up dialog sends, from the page: a same-origin POST under
  // `Referrer-Policy: no-referrer` still carries `Origin`.
  const status = await page.evaluate(async (password) => {
    const r = await fetch('/api/auth/step-up/password', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ password }),
    })
    return r.status
  }, PASSWORD)
  expect(status).toBe(204)
})

test('the pages broke no Content-Security-Policy rule', async () => {
  expect(await cspViolations()).toEqual([])
})
