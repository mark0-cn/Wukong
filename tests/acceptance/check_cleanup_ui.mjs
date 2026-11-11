// Disposable Windows runner only: exercise the actual embedded review webview.
import { chromium } from '@playwright/test';
import { spawn, execFileSync } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:net';
import { mkdtemp, mkdir, writeFile, readFile, rm, readdir, access } from 'node:fs/promises';
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
// The test relocates LOCALAPPDATA, which must not relocate the installed shared
// WebView2 runtime lookup. Use the actual installed runtime as a fixed engine.
let runtime;
for (const base of [process.env.LOCALAPPDATA, process.env['ProgramFiles(x86)'], process.env.ProgramFiles].filter(Boolean)) {
  const directory = join(base, 'Microsoft', 'EdgeWebView', 'Application');
  for (const version of await readdir(directory).catch(() => [])) {
    const candidate = join(directory, version);
    if (await access(join(candidate, 'msedgewebview2.exe')).then(() => true, () => false)) runtime = candidate;
  }
}
if (!runtime) throw Error('Installed WebView2 runtime unavailable on disposable runner');
const child = spawn(resolve(process.argv[2]), ['--json', 'cleanup', 'choose', '--output', review], {env: {...process.env, LOCALAPPDATA: join(root, 'local'), APPDATA: join(root, 'roaming'), WEBVIEW2_BROWSER_EXECUTABLE_FOLDER: runtime, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`}});
let diagnostics = '';
for (const stream of [child.stdout, child.stderr]) stream.on('data', bytes => { diagnostics = (diagnostics + bytes.toString()).slice(-4096); });
const exited = once(child, 'exit');
let browser;
try {
  const deadline = Date.now() + 45000;
  while (!browser && child.exitCode === null && Date.now() < deadline) {
    try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, {timeout: 1000}); }
    catch { await new Promise(resolve => setTimeout(resolve, 300)); }
  }
  if (!browser) {
    if (child.exitCode !== null) throw Error(`Native review exited ${child.exitCode}: ${diagnostics}`);
    console.log('CDP unavailable; checking the actual window through Windows UI Automation.');
    console.log(execFileSync('powershell.exe', ['-NoProfile', '-File', 'tests/acceptance/review-window.ps1', '-TargetProcess', String(child.pid)], {encoding: 'utf8', windowsHide: true, timeout: 35000}));
  } else {
  const page = browser.contexts()[0].pages()[0];
  await page.getByRole('heading', {name: 'Uninstall Wukong'}).waitFor();
  for (const checkbox of await page.getByRole('checkbox').all()) if (await checkbox.isChecked()) throw Error('Native cleanup option unexpectedly selected');
  await page.getByRole('checkbox', {name: 'Delete local application data and settings'}).check();
  const confirm = page.getByRole('button', {name: 'Continue uninstall'});
  await confirm.waitFor();
  await page.waitForFunction(() => ![...document.querySelectorAll('button')].find(button => button.textContent === 'Continue uninstall')?.disabled);
  await confirm.click().catch(error => { if (!/closed/i.test(String(error))) throw error; });
  }
  const [code] = await Promise.race([exited, new Promise((_, reject) => setTimeout(() => reject(Error('Native confirmation did not close the window')), 10000))]);
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
