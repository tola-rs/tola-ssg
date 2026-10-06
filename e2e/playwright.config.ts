import { defineConfig, devices } from '@playwright/test'

const isCi = process.env.CI === 'true' || process.env.CI === '1'

export default defineConfig({
  testDir: './tests',
  testMatch: '**/*.spec.ts',
  fullyParallel: true,
  forbidOnly: isCi,
  retries: 0,
  ...(isCi ? { workers: 2 } : {}),
  outputDir: 'test-results/playwright',
  preserveOutput: 'failures-only',
  reporter: isCi ? [['github'], ['html', { open: 'never', outputFolder: 'playwright-report' }]] : 'list',
  use: {
    ...devices['Desktop Chrome'],
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
  },
})
