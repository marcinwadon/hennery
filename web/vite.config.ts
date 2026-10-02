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
  test: { environment: 'jsdom', globals: true, setupFiles: './src/test-setup.ts', include: ['src/**/*.test.{ts,tsx}'] },
})
