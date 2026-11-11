// Render the current UI with illustrative data; no model, worker or native app is used.
// An optional backend capture JSON argument renders real DTOs into target/runtime-workspace/.
// Start `npm run dev`, then run `node scripts/dev/capture-readme.mjs [capture.json]`.
import { chromium } from '@playwright/test';
import { readFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { stripTypeScriptTypes } from 'node:module';
import { resolve } from 'node:path';

const root = new URL('../../', import.meta.url);
const source = await readFile(new URL('ui/src/test/fixtures.ts', root), 'utf8');
const compiled = stripTypeScriptTypes(source);
const fixtures = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`);
const { version } = JSON.parse(await readFile(new URL('tauri.conf.json', root), 'utf8'));
const snapshot = process.argv[2] ? JSON.parse(await readFile(resolve(process.argv[2]), 'utf8')) : null;
if (snapshot && (!snapshot.envelope?.workspace?.data?.transcript?.length || !snapshot.calls)) throw new Error('Expected a backend workspace capture with transcript and calls.');
const output = new URL(snapshot ? 'target/runtime-workspace/' : 'docs/assets/', root);
await mkdir(output, { recursive: true });

function demo(locale) {
  if (snapshot) return { data: snapshot.envelope.workspace.data, host: snapshot.envelope.host, calls: snapshot.calls, locale, version };
  const english = locale === 'en-US';
  const data = structuredClone(fixtures.workspace);
  const host = structuredClone(fixtures.host);
  host.model = 'analysis-model';
  host.state_dir = '/workspace/wukong';
  host.config_path = '/workspace/wukong.toml';
  const names = english
    ? ['Lead Agent', 'Parser analysis', 'Independent review']
    : ['主分析 Agent', '输入边界分析', '独立验证'];
  const tasks = english ? [
    'Recover the record format, trace input boundaries and independently check the observed behavior.',
    'Trace parse_record: recover field layout, length checks and copy operations.',
    'Replay boundary inputs and compare the results with the recorded hypothesis.',
  ] : [
    '还原记录格式，跟踪输入边界，并独立复核观察到的行为。',
    '跟踪 parse_record，确认字段布局、长度检查和内存拷贝。',
    '重放边界输入，将实际结果与记录的假设逐项比较。',
  ];
  data.runs[0].task = tasks[0];
  data.runs[1].task = english ? 'Recover the archive format and extraction flow.' : '还原归档格式与解包流程。';
  data.runs[1].summary = null;
  data.agents.forEach((agent, index) => {
    agent.name = names[index];
    agent.task = tasks[index];
    agent.scope = index === 0 ? (english ? 'Coordination' : '任务协调')
      : index === 1 ? 'parse_record' : (english ? 'Boundary replay' : '边界重放');
  });
  const inspect = {
    ...fixtures.tool,
    purpose: english ? 'Identify the file and select an analysis method.' : '确认文件身份并选择分析方法。',
    summary: 'ELF · x86_64 · entry 0x401090',
  };
  const decompile = {
    ...inspect, call_id: 'call-decompile', title: 'tool-function-decompile', target: 'parse_record',
    purpose: english ? 'Recover the length calculation and copy boundary.' : '还原长度计算与拷贝边界。',
    summary: english ? 'Recovered length check and copy path' : '已还原长度检查与拷贝路径',
    duration_ms: 1480, evidence_id: 'evidence-decompile',
    preview: { language: 'c', text: 'uint16_t size = input_length + 8;\nif (size <= sizeof(buffer))\n    memcpy(buffer, input, input_length);', truncated: true, evidence_id: 'evidence-decompile' },
  };
  const text = (value) => ({ type: 'text', text: value });
  const operation = (display, name) => ({
    type: 'tool-call', toolCallId: display.call_id, toolName: name,
    args: { display }, result: { display }, isError: false,
  });
  data.transcript = [
    { id: 'demo-plan', role: 'assistant', author: names[0], agent_id: 'root',
      created_at: '2026-11-09T03:10:01Z', content: [text(english
        ? 'I have split this into **parser analysis** and **independent review**. We will record the field layout first, then test the boundary conditions.\n\nEvery conclusion will link to its original evidence.'
        : '我已将工作拆为**输入解析分析**和**独立验证**。先记录字段布局，再验证边界条件。\n\n每项结论都会关联原始证据。')] },
    { id: 'demo-analysis', role: 'assistant', author: names[1], agent_id: 'analyst',
      created_at: '2026-11-09T03:11:10Z', content: [
        operation(inspect, 'binary_inspect'), operation(decompile, 'function_decompile'),
        text(english
          ? 'Located `parse_record`. The length check and subsequent copy use different widths.\n\nNext: compare a valid record with boundary inputs, keeping the runtime and input channel fixed.'
          : '已定位到 `parse_record`。长度检查与后续拷贝使用了不同的数值宽度。\n\n下一步：固定运行环境和输入通道，对比有效记录与边界输入。'),
      ] },
    { id: 'demo-review', role: 'assistant', author: names[2], agent_id: 'verifier',
      created_at: '2026-11-09T03:13:00Z', content: [text(english
        ? 'Preparing an independent replay with a normal-input control. The impact remains **unconfirmed** until the observed results and evidence have been checked.'
        : '正在准备独立重放，并加入正常输入对照。实际结果与原始证据核对完成前，影响保持**待确认**。')] },
  ];
  data.active_tools = [{ ...inspect, call_id: 'call-replay', title: 'tool-crash-replay',
    target: 'boundary-input.bin', state: 'running', summary: '', duration_ms: null }];
  data.findings = [{ ...fixtures.finding,
    title: english ? 'Length validation differs from the copy boundary' : '长度校验与拷贝边界不一致',
    description: english ? 'The recorded check and copy use different integer widths.' : '记录中的检查与拷贝使用不同整数宽度。',
    stage: 'suspected', severity: 'info', evidence_ids: ['evidence-decompile'],
    root_cause: english ? 'Width mismatch hypothesis; independent replay is pending.' : '整数宽度不一致的假设，等待独立重放。',
    severity_rationale: english ? 'An unverified hypothesis from recovered code.' : '来自代码还原的待验证假设。',
    impact: english ? 'Pending independent replay.' : '等待独立重放。',
  }];
  data.notes = [];
  return { data, host, locale, inspect, decompile, version };
}

const browser = await chromium.launch();
try {
  for (const locale of ['en-US', 'zh-CN']) {
    const context = await browser.newContext({ viewport: { width: 1680, height: 1100 },
      deviceScaleFactor: 1, locale, timezoneId: 'UTC', reducedMotion: 'reduce' });
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    const input = demo(locale);
    await page.addInitScript(({ data, host, locale, inspect, decompile, calls, version }) => {
      const callbacks = new Map();
      let next = 0;
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback(callback) { callbacks.set(++next, callback); return next; },
        unregisterCallback(id) { callbacks.delete(id); },
        async invoke(command, args = {}) {
          if (command === 'preferences_read') return { ui_language: locale, output_language: 'auto', system_locale: locale };
          if (command === 'plugin:event|listen') return ++next;
          if (command === 'plugin:event|unlisten') return;
          if (command === 'workspace_read') {
            const snapshot = structuredClone(data);
            if (args.request.agent) snapshot.transcript = snapshot.transcript.filter(row => row.agent_id === args.request.agent);
            const revision = `demo:${args.request.agent ?? 'all'}`;
            return { host, workspace: { revision, data: args.request.known_revision === revision ? null : snapshot } };
          }
          if (command === 'tool_details') {
            if (calls) {
              const call = calls.find(call => call.id === args.id);
              if (!call) throw new Error(`Backend capture has no call ${args.id}`);
              return call;
            }
            const display = args.id === decompile.call_id ? decompile : inspect;
            return { id: display.call_id, agent_id: 'analyst', name: args.id === decompile.call_id ? 'function_decompile' : 'binary_inspect',
              arguments: { target: display.target }, result: { summary: display.summary }, evidence_id: display.evidence_id, state: 'completed' };
          }
          if (command === 'update_action') return { version, phase: 'idle', message: '', available: null,
            notes: '', downloaded: 0, total: null, prepared: null, recovery: null, needs_setup: false, pending: false };
          throw new Error(`Screenshot adapter does not implement: ${command}`);
        },
      };
    }, input);
    await page.goto('http://127.0.0.1:1420', { waitUntil: 'networkidle' });
    await page.getByRole('treeitem').nth(input.data.agents.length - 1).waitFor();
    await page.locator('.message').first().waitFor();
    await page.evaluate(() => document.fonts.ready);
    await page.locator('details[open]').evaluateAll(elements => elements.forEach(element => { element.open = false; }));
    await page.locator('.transcript').evaluate(element => { element.scrollTop = 0; });
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const clipped = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
    if (clipped) throw new Error(`The ${locale} preview overflows the viewport.`);
    if (await page.getByRole('alert').count()) throw new Error((await page.getByRole('alert').allTextContents()).join('\n'));
    if (errors.length) throw new Error(errors.join('\n'));
    await page.mouse.move(1675, 1095);
    await page.screenshot({ path: fileURLToPath(new URL(`workspace.${locale}.png`, output)), animations: 'disabled' });
    console.log(`Captured ${snapshot ? 'target/runtime-workspace' : 'docs/assets'}/workspace.${locale}.png (${snapshot ? snapshot.source : 'current UI, illustrative task data'}).`);
    await context.close();
  }
} finally {
  await browser.close();
}
