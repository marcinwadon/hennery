// Browser checks of the shell against the real binary (plan 4b; client view
// spec §5.3: jsdom cannot see the cascade). Chromium only, from the flake
// (`PLAYWRIGHT_BROWSERS_PATH`): never `playwright install`.
import { defineConfig, devices } from '@playwright/test'

export default defineConfig({
  testDir: 'e2e',
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? 'list' : 'line',
  timeout: 60_000,
  use: { ...devices['Desktop Chrome'], headless: true },
})
