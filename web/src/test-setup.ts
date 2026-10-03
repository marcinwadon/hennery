import '@testing-library/jest-dom/vitest'
import { configure } from '@testing-library/react'

// `findBy*` and `waitFor` give up after 1 s by default: on a loaded machine
// a whole app's first render can take longer. Kept well below the test
// timeout (vite.config.ts), so a wait that never ends fails with its own
// message, not as a timed-out test.
configure({ asyncUtilTimeout: 5000 })
