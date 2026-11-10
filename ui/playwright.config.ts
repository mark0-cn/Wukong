import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';

const projectRoot = fileURLToPath(new URL('..', import.meta.url));
export default defineConfig({
  testDir: './e2e',
  outputDir: '../test-results',
  fullyParallel: false,
  workers: 1,
  reporter: 'list',
  use: { baseURL: 'http://127.0.0.1:1420', viewport: { width: 1440, height: 1000 }, trace: 'retain-on-failure' },
  webServer: {
    command: 'npm run dev', cwd: projectRoot,
    url: 'http://127.0.0.1:1420', reuseExistingServer: !process.env.CI,
  },
});
