/// <reference types="vitest/config" />
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// Tests run in a zone with daylight saving time, so the session list's day
// buckets are tested across a 23 h and a 25 h day (lib/time.test.ts). The
// test workers inherit it from this process.
process.env.TZ = 'Europe/Warsaw'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  build: { outDir: 'dist', target: 'es2022' },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: './src/test-setup.ts',
    include: ['src/**/*.test.{ts,tsx}'],
    // A loaded machine (CI's 2–4 vCPUs, or parallel runs) takes seconds to
    // import a test's module graph and render a whole app: the 5 s default
    // timed out tests that pass. A real hang still fails, only later.
    testTimeout: 20000,
  },
})
