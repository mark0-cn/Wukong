import { test, expect } from '@playwright/test';

test('uninstall review defaults to retaining data/Docker and ignores stale previews', async ({page}) => {
  await page.addInitScript(() => {
    const requests: string[] = [];
    Object.assign(window, {__WUKONG_CLEANUP__: {initialData: false}, __cleanupRequests: requests,
      __TAURI_INTERNALS__: {invoke: async (command: string, args: any = {}) => {
        requests.push(command);
        if (command === 'preferences_read') return {ui_language: 'en-US', output_language: 'auto', system_locale: 'en-US'};
        if (command === 'cleanup_preview') {
          if (args.selection.tools) throw Error('Docker is unavailable');
          await new Promise(resolve => setTimeout(resolve, args.selection.data ? 30 : 200));
          return {fingerprint: args.selection.data ? 'data' : 'empty', files: args.selection.data ? 4 : 0, bytes: 1048576,
            paths: args.selection.data ? ['C:\\Data\\Wukong'] : [], resources: [], retained: [{path: 'original.elf', reason: 'external original'}], errors: [], truncated: false};
        }
        if (command === 'cleanup_confirm') throw Error('cleanup contents changed; review a fresh plan');
      }} });
  });
  await page.goto('/');
  const data = page.getByRole('checkbox', {name: 'Delete local application data and settings'});
  const tools = page.getByRole('checkbox', {name: 'Delete Docker tool environments'});
  await expect(data).not.toBeChecked();
  await expect(tools).not.toBeChecked();
  await expect(page.getByRole('checkbox', {name: 'Delete Docker challenge environments'})).not.toBeChecked();
  await data.check();
  await expect(page.getByText('4 files · 1.0 MiB of local files · 0 Docker resources listed')).toBeVisible();
  await page.waitForTimeout(250);
  await expect(page.getByText('C:\\Data\\Wukong', {exact: true})).toBeVisible();
  await tools.check();
  await expect(page.getByRole('alert')).toHaveText('Docker is unavailable');
  await expect(page.getByRole('button', {name: 'Continue uninstall'})).toBeDisabled();
  await tools.uncheck();
  await expect(page.getByRole('button', {name: 'Continue uninstall'})).toBeEnabled();
  await page.getByRole('button', {name: 'Continue uninstall'}).click();
  await expect(page.getByRole('alert')).toContainText('contents changed');
  await page.getByRole('button', {name: 'Cancel uninstall'}).click();
  expect(await page.evaluate(() => (window as any).__cleanupRequests)).not.toContain('workspace_read');
  expect(await page.evaluate(() => (window as any).__cleanupRequests)).toContain('cleanup_cancel');
  await page.screenshot({path: 'test-results/uninstall-review.png'});
});
