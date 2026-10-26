import { defineConfig, devices } from '@playwright/test';

// End-to-end proof runs against the demo backend (synthetic data, see src/demo).
export default defineConfig({
  testDir: './tests',
  outputDir: './test-results/output',
  fullyParallel: true,
  workers: 4,
  reporter: [['list']],
  use: { baseURL: 'http://127.0.0.1:5180', trace: 'retain-on-failure' },
  projects: [
    { name: 'desktop', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 900 } } },
    { name: 'phone', use: { ...devices['iPhone 14'], browserName: 'chromium' } },
  ],
  webServer: {
    command: 'npx vite --host 127.0.0.1 --port 5180 --strictPort',
    url: 'http://127.0.0.1:5180',
    reuseExistingServer: true,
    env: { VITE_DEMO: '1' },
  },
});
