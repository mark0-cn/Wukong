// Actual embedded review, only on a disposable Windows GitHub runner.
import { spawn, execFileSync } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, writeFile, readFile, rm, readdir, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join, sep } from 'node:path';
if (process.env.GITHUB_ACTIONS !== 'true' || process.env.RUNNER_ENVIRONMENT !== 'github-hosted' || process.platform !== 'win32') throw Error('Disposable Windows GitHub runner required');
const root = resolve(await mkdtemp(join(tmpdir(), 'wukong-review-')));
const home = join(root, 'local', 'WukongData');
const temporary = join(root, 'temp');
await mkdir(home, {recursive: true});
await mkdir(temporary);
const settings = join(home, 'preferences.json');
await writeFile(settings, JSON.stringify({ui_language: 'en-US', output_language: 'auto'}));
const review = join(root, 'review.json');
// Relocating the test profile must not relocate the shared WebView2 runtime.
let runtime;
for (const base of [process.env.LOCALAPPDATA, process.env['ProgramFiles(x86)'], process.env.ProgramFiles].filter(Boolean)) {
  const directory = join(base, 'Microsoft', 'EdgeWebView', 'Application');
  for (const version of await readdir(directory).catch(() => [])) {
    const candidate = join(directory, version);
    if (await access(join(candidate, 'msedgewebview2.exe')).then(() => true, () => false)) runtime = candidate;
  }
}
if (!runtime) throw Error('Installed WebView2 runtime unavailable on disposable runner');
const child = spawn(resolve(process.argv[2]), ['--json', 'cleanup', 'choose', '--output', review], {env: {...process.env, LOCALAPPDATA: join(root, 'local'), APPDATA: join(root, 'roaming'), TEMP: temporary, TMP: temporary, WEBVIEW2_BROWSER_EXECUTABLE_FOLDER: runtime}});
let diagnostics = '';
for (const stream of [child.stdout, child.stderr]) stream.on('data', bytes => { diagnostics = (diagnostics + bytes.toString()).slice(-4096); });
const exited = once(child, 'exit');
try {
  console.log(execFileSync('powershell.exe', ['-NoProfile', '-STA', '-File', 'tests/acceptance/review-window.ps1', '-TargetProcess', String(child.pid)], {encoding: 'utf8', windowsHide: true, timeout: 35000}));
  const [code] = await Promise.race([exited, new Promise((_, reject) => setTimeout(() => reject(Error('Native confirmation did not close the window')), 10000))]);
  if (code !== 33) throw Error(`Native confirmation returned ${code}: ${diagnostics}`);
  const plan = JSON.parse(await readFile(review, 'utf8'));
  if (!plan.selection.data || plan.selection.tools || plan.selection.challenges) throw Error('Native review scope differs from selected checkboxes');
  await readFile(settings); // Review must not delete application data.
  if ((await readdir(temporary)).some(name => name.startsWith('wukong-review-'))) throw Error('Review browser profile was retained after confirmation');
  console.log('Actual WebView review, scope, exit protocol and temporary-profile cleanup passed.');
} catch (error) {
  console.error(`Native startup diagnostics: ${diagnostics}`);
  throw error;
} finally {
  if (child.exitCode === null) { child.kill(); await exited; }
  if (!root.startsWith(resolve(tmpdir()) + sep) || !root.split(sep).at(-1).startsWith('wukong-review-')) throw Error('Unexpected test cleanup path');
  await rm(root, {recursive: true, force: true, maxRetries: 10, retryDelay: 200});
}
