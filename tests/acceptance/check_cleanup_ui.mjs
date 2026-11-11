// Disposable Windows runner only: exercise the actual embedded review webview.
import { chromium } from '@playwright/test';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:net';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join, sep } from 'node:path';
if (process.env.GITHUB_ACTIONS !== 'true' || process.env.RUNNER_ENVIRONMENT !== 'github-hosted' || process.platform !== 'win32') throw Error('Disposable Windows GitHub runner required');
const root = resolve(await mkdtemp(join(tmpdir(), 'wukong-review-')));
const server = createServer().listen(0, '127.0.0.1');
await once(server, 'listening');
const port = server.address().port;
server.close();
const home = join(root, 'local', 'WukongData');
await mkdir(home, {recursive: true});
const settings = join(home, 'preferences.json');
await writeFile(settings, JSON.stringify({ui_language: 'en-US', output_language: 'auto'}));
const review = join(root, 'review.json');
const child = spawn(resolve(process.argv[2]), ['cleanup', 'choose', '--output', review], {env: {...process.env, LOCALAPPDATA: join(root, 'local'), APPDATA: join(root, 'roaming'), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`}});
const exited = once(child, 'exit');
let browser;
try {
  const deadline = Date.now() + 45000;
  while (!browser && Date.now() < deadline) {
    try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, {timeout: 1000}); }
    catch { await new Promise(resolve => setTimeout(resolve, 300)); }
  }
  if (!browser) throw Error('Native cleanup webview did not open');
  const page = browser.contexts()[0].pages()[0];
  await page.getByRole('heading', {name: 'Uninstall Wukong'}).waitFor();
  for (const checkbox of await page.getByRole('checkbox').all()) if (await checkbox.isChecked()) throw Error('Native cleanup option unexpectedly selected');
  await page.getByRole('checkbox', {name: 'Delete local application data and settings'}).check();
  const confirm = page.getByRole('button', {name: 'Continue uninstall'});
  await confirm.waitFor();
  await page.waitForFunction(() => ![...document.querySelectorAll('button')].find(button => button.textContent === 'Continue uninstall')?.disabled);
  await confirm.click();
  const [code] = await exited;
  if (code !== 33) throw Error(`Native confirmation returned ${code}; expected data-only scope 33`);
  const plan = JSON.parse(await readFile(review, 'utf8'));
  if (!plan.selection.data || plan.selection.tools || plan.selection.challenges) throw Error('Native review scope differs from selected checkboxes');
  await readFile(settings); // Review itself must never delete application data.
  console.log('Actual WebView review, default retention, selected scope and exit protocol passed.');
} finally {
  if (child.exitCode === null) { child.kill(); await exited; }
  await browser?.close().catch(() => {});
  if (!root.startsWith(resolve(tmpdir()) + sep) || !root.split(sep).at(-1).startsWith('wukong-review-')) throw Error('Unexpected test cleanup path');
  await rm(root, {recursive: true, force: true});
}
