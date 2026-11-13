import { defineConfig, devices } from '@playwright/test';

// create-playwright's template config (assets/playwright.config.ts), with what
// the benchmark changes: only the chromium project; the 'list' reporter
// instead of 'html'; trace off; no retries; workers from the command line;
// and the browser binary bitti is given (BENCH_CHROMIUM), headless.
const chromium = process.env.BENCH_CHROMIUM;

export default defineConfig({
  testDir: './tests',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: undefined,
  reporter: 'list',
  use: {
    trace: 'off',
    headless: true,
  },
  projects: [
    {
      name: 'chromium',
      use: {
        ...devices['Desktop Chrome'],
        launchOptions: chromium ? { executablePath: chromium } : {},
      },
    },
  ],
});
