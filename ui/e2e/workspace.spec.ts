import { test, expect, type Page } from '@playwright/test';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { agents, budget, finding, host, tool, workspace } from '../src/test/fixtures';

async function mockDesktop(page: Page) {
  await page.addInitScript(({ data, host, tool, finding }) => {
    const state = { data, host, preferences: { ui_language: 'zh-CN', output_language: 'auto', system_locale: 'en-US' }, requests: [] as { command: string; args: Record<string, any> }[], revision: 1, sendDelay: 0, preparedUpdate: null as string | null };
    const callbacks = new Map<number, (value: unknown) => void>();
    const listeners = new Map<string, number[]>();
    let next = 0;
    const emit = (event: string, payload: unknown) => (listeners.get(event) ?? []).forEach(id => callbacks.get(id)?.({ event, payload, id }));
    Object.assign(window, { __desktopTest: state, __emitDesktop: emit, __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} }, __TAURI_INTERNALS__: {
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
      transformCallback: (callback: (value: unknown) => void) => { callbacks.set(++next, callback); return next; },
      unregisterCallback: (id: number) => callbacks.delete(id),
      invoke: async (command: string, args: Record<string, any> = {}) => {
        state.requests.push({ command, args });
        if (command === 'preferences_read') return JSON.parse(sessionStorage.getItem('test-preferences') ?? 'null') ?? state.preferences;
        if (command === 'preferences_save') { state.preferences = { ...state.preferences, ...args.preferences }; sessionStorage.setItem('test-preferences', JSON.stringify(state.preferences)); return state.preferences; }
        if (command === 'update_action') {
          if (args.action === 'prepare') state.preparedUpdate = '/cache/verified-update';
          return { version: '0.2.0', phase: 'idle', message: '当前工具环境已就绪', available: '0.2.1', notes: '修复工具兼容性', downloaded: 0, total: null, prepared: state.preparedUpdate, recovery: null, needs_setup: false, pending: false };
        }
        if (command === 'update_storage') return { data_dir: '/data/workspace', cache_dir: '/data/cache' };
        if (command === 'workbench_action') return { state: 'ready' };
        if (command === 'plugin:event|listen') { listeners.set(args.event, [...(listeners.get(args.event) ?? []), args.handler]); return args.handler; }
        if (command === 'plugin:event|unlisten') { listeners.set(args.event, (listeners.get(args.event) ?? []).filter(id => id !== args.eventId)); return; }
        if (command === 'workspace_read') {
          const request = args.request;
          const snapshot = structuredClone(state.data);
          snapshot.selected = request.run ?? snapshot.selected;
          if (snapshot.selected === 'run-b') { snapshot.agents = [{ ...snapshot.agents[0], id: 'root-b', name: '归档分析 Agent' }]; snapshot.transcript = []; snapshot.active_tools = []; }
          else if (request.agent) snapshot.transcript = snapshot.transcript.filter(message => message.agent_id === request.agent);
          const revision = `${state.revision}:${snapshot.selected}:${request.agent ?? ''}:${request.event_limit}`;
          return { host: structuredClone(state.host), workspace: { revision, data: revision === request.known_revision ? null : snapshot } };
        }
        if (command === 'tool_details') return { id: args.id, agent_id: 'root', name: 'binary_info', arguments: { path: 'parser.elf', raw_parameter: 'visible-on-demand' }, result: { architecture: 'x86_64', format: 'elf' }, evidence_id: tool.evidence_id, state: 'completed' };
        if (command === 'send_message') {
          await new Promise(resolve => setTimeout(resolve, state.sendDelay));
          state.data.transcript.push({ id: `mail-${state.revision}`, role: 'user', author: `你 → ${args.agent}`, agent_id: args.agent, created_at: new Date().toISOString(), delivery: 'queued', content: [{ type: 'text', text: args.text }] });
          state.revision++; emit('workspace-changed', null); return { delivery: 'queued', recipient_generation: args.generation };
        }
        if (command === 'finding_details') return { finding, verification: null, evidence: [{ id: 'evidence-a', agent_id: 'analyst', tool_call_id: 'call-copy', source: 'reproduction.txt', media_type: 'text/plain', size: 20000, sha256: 'fixture-sha256' }], calls: [], missing_evidence: [] };
        if (command === 'evidence_preview') return { id: args.id, source: 'reproduction.txt', media_type: 'text/plain', size: 20000, sha256: 'fixture-sha256', offset: args.offset, end: args.offset === 0 ? 16384 : 20000, text: args.offset === 0 ? 'first evidence page: controlled reproduction' : 'second evidence page: end of output', binary: false };
        if (command === 'config_read') return { path: '/tmp/wukong.toml', source: '# retain this comment\n[model]\nmodel = "gpt-5.2"\n', original: '# retain this comment\n[model]\nmodel = "gpt-5.2"\n', form: { model: { provider: 'openai', model: 'gpt-5.2' }, budget: state.host.budget, state_dir: 'state' } };
        if (command === 'config_save') { state.host = { ...state.host, epoch: state.host.epoch + 1, execution: null }; return { ...args.document, original: args.document.source }; }
        if (command === 'pause_run') { state.host.execution = null; state.data.runs[0].status = 'budget_paused'; state.revision++; emit('workspace-changed', null); return; }
        if (command === 'pick_target') return '/samples/new.elf';
        if (command === 'start_run' || command === 'resume_run') return 'run-a';
        if (command === 'export_report') return '/reports/fixture';
        if (command === 'stop_agent' || command === 'shutdown_workspace') return;
        throw new Error(`Unhandled test command: ${command}`);
      },
    } });
  }, { data: workspace, host, tool, finding });
}
test.beforeEach(async ({ page }) => { await mockDesktop(page); });

test('public text, evidence code and background jobs update the existing conversation', async ({ page }) => {
  await page.goto('/');
  await page.getByLabel('发送给 Agent 的消息').fill('keep my analysis draft');
  await page.evaluate(() => {
    const state = (window as any).__desktopTest;
    const code = 'uint16_t size = input_length + 8;\nif (size <= sizeof(buffer))\n    memcpy(buffer, input, input_length);';
    const decompile = { ...state.data.active_tools[0], call_id: 'decompile-real-shape', title: 'tool-function-decompile', state: 'completed', duration_ms: 980,
      summary: { code: 'result-decompiled', args: { name: 'parse_record', chars: code.length } },
      preview: { language: 'c', text: code, truncated: true, evidence_id: 'original-ghidra-output' } };
    const replay = { ...decompile, call_id: 'replay-job-origin', title: 'tool-crash-replay', state: 'running', duration_ms: null,
      preview: undefined, summary: { code: 'progress-job-background', args: {} }, job: { id: 'worker-replay', state: 'running', stale: false,
        started_at: new Date(Date.now()-1000).toISOString(), finished_at: null, observed_at: new Date().toISOString(), attempts: null, successes: null } };
    const operation = (display: any, toolName: string) => ({ type: 'tool-call', toolCallId: display.call_id, toolName, args: { display }, result: { display }, isError: false });
    state.data.transcript = [
      { id: 'lead-text', role: 'assistant', author: '主分析 Agent', agent_id: 'root', created_at: '2026-11-11T03:10:00Z', content: [{ type: 'text', text: '先还原解析逻辑，再独立重放边界输入。' }] },
      { id: 'analyst-code', role: 'assistant', author: '输入边界分析', agent_id: 'analyst', created_at: '2026-11-11T03:11:00Z', content: [operation(decompile, 'function_decompile')] },
      { id: 'reply-stable', role: 'assistant', author: '独立验证', agent_id: 'verifier', state: 'streaming', created_at: '2026-11-11T03:12:00Z', content: [{ type: 'text', text: '正在独立重放' }, operation(replay, 'crash_replay')] },
    ];
    state.data.active_tools = [replay];
    state.revision++; (window as any).__emitDesktop('workspace-changed', null);
  });
  await expect(page.locator('.work-status')).toContainText('重放');
  await expect(page.locator('.tool-code-preview')).toContainText('memcpy(buffer, input, input_length);');
  await expect(page.locator('.tool-code-preview')).toContainText('original-ghidra-output');
  await expect(page.getByText('正在输出…', { exact: true })).toBeVisible();
  await expect(page.getByLabel('发送给 Agent 的消息')).toHaveValue('keep my analysis draft');
  await page.evaluate(() => {
    const state = (window as any).__desktopTest;
    const message = state.data.transcript.find((row: any) => row.id === 'reply-stable');
    message.state = 'complete'; message.content[0].text = '独立重放完成：两次输入中一次符合预期，验证未通过。';
    const display = message.content[1].args.display;
    display.state = 'failed'; display.duration_ms = 4200; display.summary = { code: 'result-failed', args: {} };
    Object.assign(display.job, { state: 'completed', attempts: 2, successes: 1, finished_at: new Date().toISOString() });
    state.data.active_tools = []; state.revision++; (window as any).__emitDesktop('workspace-changed', null);
  });
  await expect(page.getByText('正在输出…', { exact: true })).toHaveCount(0);
  await expect(page.getByText('独立重放完成：两次输入中一次符合预期，验证未通过。', { exact: true })).toHaveCount(1);
  await expect(page.locator('.tool-failed')).toContainText('2 次尝试');
  await expect(page.locator('.tool-failed')).toContainText('1 次试验成功');
  await expect(page.locator('.tool-failed')).toContainText('4.2s');
  await expect(page.getByLabel('发送给 Agent 的消息')).toHaveValue('keep my analysis draft');
  expect(await page.evaluate(() => (window as any).__desktopTest.requests.filter((row: any) => row.command === 'tool_details').length)).toBe(0);
  await page.screenshot({ path: 'target/live-analysis-progress.png' });
  await page.locator('.tool-failed .tool-trigger').click();
  await expect(page.locator('.tool-failed .tool-details')).toContainText('请求状态');
  await expect(page.locator('.tool-failed .tool-details')).toContainText('已完成');
  await expect(page.locator('.tool-failed .tool-details')).toContainText('worker-replay');
});

test('language changes preserve active work, drafts and configuration identity', async ({ page }) => {
  await page.goto('/');
  await page.getByLabel('发送给 Agent 的消息').fill('keep this exact payload: 原始字节');
  await page.evaluate(() => {
    const native = (window as any).__TAURI_INTERNALS__;
    const invoke = native.invoke;
    native.invoke = (command: string, args: unknown) => command === 'export_report'
      ? Promise.reject({ code: 'error-operation', args: {}, detail: 'original diagnostic: 0x4141 原始字节' }) : invoke(command, args);
    (window as any).__desktopTest.host.notice = { id: 123, error: false, message: { code: 'system-an-update-is-available', args: {} } };
    (window as any).__emitDesktop('workspace-changed', null);
  });
  await page.getByRole('button', { name: '导出', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('操作未能完成');
  await page.getByRole('button', { name: '工作区设置', exact: true }).click();
  await page.getByLabel('主模型', { exact: true }).fill('draft-model');
  await page.getByLabel('界面语言', { exact: true }).selectOption('en-US');
  await expect(page.getByRole('heading', { name: 'Workspace settings' })).toBeVisible();
  await expect(page.getByLabel('Main model', { exact: true })).toHaveValue('draft-model');
  await expect(page.getByLabel('Interface language')).toHaveValue('en-US');
  await page.getByLabel('New task output language').selectOption('zh-CN');
  await page.getByLabel('Close', { exact: true }).click();
  await expect(page.getByLabel('Message to Agent')).toHaveValue('keep this exact payload: 原始字节');
  await expect(page.getByRole('button', { name: 'Pause', exact: true })).toBeVisible();
  await expect(page.getByText('Trace cross-references', { exact: true })).toBeVisible();
  await expect(page.getByRole('alert')).toContainText('The operation could not be completed.');
  await expect(page.getByRole('alert')).toContainText('original diagnostic: 0x4141 原始字节');
  await expect(page.getByText('An update is available.', { exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.lang)).toBe('en-US');
  const state = await page.evaluate(() => (window as any).__desktopTest);
  expect(state.host.epoch).toBe(host.epoch);
  expect(state.requests.some((r: any) => ['pause_run', 'config_save', 'start_run', 'resume_run'].includes(r.command))).toBe(false);
  expect(state.data.transcript).toEqual(workspace.transcript);
  await page.getByRole('button', { name: 'Workspace settings', exact: true }).click();
  await expect(page.getByLabel('Main model', { exact: true })).toHaveValue('draft-model');
  await page.getByLabel('Interface language').selectOption('zh-CN');
  await expect(page.getByLabel('主模型', { exact: true })).toHaveValue('draft-model');
});

test('English interface fits the minimum desktop and reload keeps language preference', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: '工作区设置', exact: true }).click();
  await page.getByLabel('界面语言').selectOption('en-US');
  await page.getByLabel('Close', { exact: true }).click();
  await page.reload();
  await page.setViewportSize({ width: 860, height: 600 });
  await expect(page.getByRole('button', { name: /^New analysis/ })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: 'target/i18n-english-minimum.png' });
});

test('close requests use current draft state and preserve text when dismissed', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'parser.elf', exact: true })).toBeVisible();
  await page.evaluate(() => { (window as any).__desktopTest.host.execution = null; (window as any).__emitDesktop('workspace-changed', null); });
  await expect(page.getByRole('button', { name: '暂停分析', exact: true })).toHaveCount(0);
  await page.getByLabel('发送给 Agent 的消息').fill('unsent evidence note');
  await page.evaluate(() => (window as any).__emitDesktop('close-requested', { busy: false }));
  await expect(page.getByRole('dialog')).toBeVisible();
  expect(await page.evaluate(() => (window as any).__desktopTest.requests.some((request: any) => request.command === 'shutdown_workspace'))).toBe(false);
  await page.getByRole('button', { name: '取消', exact: true }).click();
  await expect(page.getByLabel('发送给 Agent 的消息')).toHaveValue('unsent evidence note');
});

test('native drop opens a task with the selected target', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'parser.elf', exact: true })).toBeVisible();
  await page.evaluate(() => (window as any).__emitDesktop('tauri://drag-drop', { paths: ['/samples/drop.elf'], position: { x: 10, y: 10 } }));
  await expect(page.getByRole('dialog')).toBeVisible();
  await expect(page.getByLabel('目标文件路径')).toHaveValue('/samples/drop.elf');
});
test('shows public progress and paired tool results without raw JSON', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'parser.elf', exact: true })).toBeVisible();
  await expect(page.getByText('启动调试会话', { exact: true })).toBeVisible();
  await expect(page.getByText('调试后端暂不可用：连接已关闭。已保留当前证据。')).toBeVisible();
  await expect(page.getByText('raw_parameter')).toHaveCount(0);
  await page.locator('.transcript').evaluate(element => { element.scrollTop = 0; });
  await expect(page.getByText('检查文件格式与安全属性', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: /检查文件格式与安全属性/ }).click();
  await expect(page.getByText(/visible-on-demand/)).toBeVisible();
  await expect(page.locator('.agent-card')).toHaveCount(3);
  expect(errors).toEqual([]);
  await page.screenshot({ path: join(tmpdir(), 'wukong-web-transcript.png'), fullPage: true });
});
test('retains per-agent drafts and targets message delivery after navigation', async ({ page }) => {
  await page.goto('/');
  await page.locator('.agent-card').filter({ hasText: '输入边界分析' }).click();
  await page.getByLabel('发送给 Agent 的消息').fill('draft for analyst');
  await page.locator('.agent-card').filter({ hasText: '复现与独立验证' }).click();
  await page.getByLabel('发送给 Agent 的消息').fill('draft for verifier');
  await page.locator('.agent-card').filter({ hasText: '输入边界分析' }).click();
  await expect(page.getByLabel('发送给 Agent 的消息')).toHaveValue('draft for analyst');
  await page.evaluate(() => { (window as any).__desktopTest.sendDelay = 1000; });
  await page.getByLabel('发送消息', { exact: true }).click();
  await page.locator('.agent-card').filter({ hasText: '复现与独立验证' }).click();
  await page.locator('.agent-card').filter({ hasText: '输入边界分析' }).click();
  await page.getByLabel('发送给 Agent 的消息').fill('newer analyst draft');
  await page.waitForFunction(() => (window as any).__desktopTest.data.transcript.some((message: any) => message.id.startsWith('mail-')));
  await expect(page.getByLabel('发送给 Agent 的消息')).toHaveValue('newer analyst draft');
  const sent = await page.evaluate(() => (window as any).__desktopTest.requests.find((request: any) => request.command === 'send_message'));
  expect(sent.args.agent).toBe('analyst'); expect(sent.args.generation).toBe(agents[1].generation);
});
test('loads scoped evidence pages and preserves settings drafts', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: /发现与证据/ }).click();

  await expect(page.getByText('first evidence page: controlled reproduction')).toBeVisible();
  await page.getByLabel('下一页证据').click();
  await expect(page.getByText('second evidence page: end of output')).toBeVisible();
  await page.getByRole('button', { name: '工作区设置', exact: true }).click();
  await page.getByLabel('主模型', { exact: true }).fill('draft-model');
  await page.getByRole('button', { name: '关闭', exact: true }).last().click();
  await page.getByRole('button', { name: '工作区设置', exact: true }).click();
  await expect(page.getByLabel('主模型', { exact: true })).toHaveValue('draft-model');
  await expect(page.getByRole('button', { name: '保存并应用' })).toBeDisabled();
});

 test('update center presents versions and requires preparation before restart', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: '更新', exact: true }).click();
  await expect(page.getByText('0.2.0', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '安装并重启', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: '下载并验证更新', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: '下载并验证更新', exact: true }).click();
  await expect(page.getByRole('button', { name: '安装并重启', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: '安装并重启', exact: true }).click();
  await expect(page.getByRole('button', { name: '确认', exact: true })).toBeVisible();
  const requests = await page.evaluate(() => (window as any).__desktopTest.requests);
  expect(requests.some((r: any) => r.command === 'update_action' && r.args.action === 'install')).toBe(false);
  await page.screenshot({ path: join(tmpdir(), 'wukong-update-center.png'), fullPage: true });
});

test('focus mode restores side panels without losing the current draft', async ({ page }) => {
  await page.goto('/');
  const composer = page.getByLabel('发送给 Agent 的消息');
  await composer.fill('保留专注模式前的草稿');
  await page.getByRole('button', { name: '专注模式', exact: true }).click();
  await expect(page.locator('.sidebar')).toHaveCount(0);
  await expect(page.locator('.agents-panel')).toHaveCount(0);
  await expect(composer).toHaveValue('保留专注模式前的草稿');
  await page.getByRole('button', { name: '专注模式', exact: true }).click();
  await expect(page.locator('.sidebar')).toBeVisible();
  await expect(page.locator('.agents-panel')).toBeVisible();
  await expect(composer).toHaveValue('保留专注模式前的草稿');
  await page.getByRole('button', { name: '收起协作任务', exact: true }).click();
  await page.getByRole('button', { name: '专注模式', exact: true }).click();
  await page.getByRole('button', { name: '专注模式', exact: true }).click();
  await expect(page.locator('.agents-panel')).toHaveCount(0);
});

test('keyboard resizing persists and minimum desktop size keeps controls reachable', async ({ page }) => {
  await page.goto('/');
  const width = () => page.locator('.sidebar').evaluate(el => el.getBoundingClientRect().width);
  const initial = await width();
  await page.getByRole('separator', { name: '调整任务列表宽度' }).focus();
  await page.keyboard.press('ArrowRight');
  await expect.poll(width).toBeGreaterThan(initial + 5);
  const resized = await width();
  await page.reload();
  await expect.poll(width).toBeCloseTo(resized, 0);
  await page.setViewportSize({ width: 860, height: 600 });
  await expect(page.locator('.agents-panel')).toHaveCount(0);
  await expect(page.getByLabel('发送消息', { exact: true })).toBeInViewport();
  await expect(page.getByRole('button', { name: '更新', exact: true })).toBeInViewport();
  await page.getByRole('button', { name: '发现与证据', exact: false }).click();
  await page.locator('.finding-card').first().click();

  await expect(page.getByText('first evidence page: controlled reproduction')).toBeVisible();
  await page.getByLabel('下一页证据').scrollIntoViewIfNeeded();
  await expect(page.getByLabel('下一页证据')).toBeInViewport();
  await page.getByLabel('下一页证据').click();
  await expect(page.getByText('second evidence page: end of output')).toBeVisible();
  await page.screenshot({ path: join(tmpdir(), 'wukong-workspace-narrow.png'), fullPage: true });
  await page.getByRole('button', { name: '返回发现列表', exact: true }).click();
  await expect(page.locator('.finding-card').first()).toBeVisible();
});

test('task names, filters and panel choices survive reload without changing tasks', async ({ page }) => {
  await page.goto('/');
  await page.getByLabel('重命名任务', { exact: true }).click();
  await page.getByLabel('任务名称', { exact: true }).fill('parser 第一轮');
  await page.getByRole('button', { name: '保存名称', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'parser 第一轮', exact: true })).toBeVisible();
  await page.getByLabel('筛选任务').selectOption('history');
  await expect(page.locator('.run-card')).toHaveCount(1);
  await page.getByLabel('筛选任务').selectOption('all');
  await page.getByRole('button', { name: '收起协作任务', exact: true }).click();
  await page.reload();
  await expect(page.getByRole('heading', { name: 'parser 第一轮', exact: true })).toBeVisible();
  await expect(page.locator('.agents-panel')).toHaveCount(0);
  const state = await page.evaluate(() => (window as any).__desktopTest);
  expect(state.data.runs[0].target_path).toBe(workspace.runs[0].target_path);
  expect(state.data.runs[0].task).toBe(workspace.runs[0].task);
});

test('latest-progress affordance follows scrolling and connection failure changes its indicator', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByLabel('发送给 Agent 的消息')).toBeVisible();
  await expect(page.getByLabel('跳到最新进展')).toHaveCount(0);
  await page.locator('.transcript').evaluate(element => { element.scrollTop = 0; });
  await expect(page.locator('.jump-latest')).toBeVisible();
  await page.locator('.jump-latest').click();
  await expect(page.locator('.jump-latest')).toHaveCount(0);
  await page.evaluate(() => {
    const native = (window as any).__TAURI_INTERNALS__;
    const invoke = native.invoke;
    native.invoke = (command: string, args: unknown) => command === 'workspace_read' ? Promise.reject('connection unavailable') : invoke(command, args);
    (window as any).__emitDesktop('workspace-changed', null);
  });
  await expect(page.locator('.connection-error')).toBeVisible();
});

test('settings categories retain drafts and separate tool and challenge actions', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: '工作区设置', exact: true }).click();
  await page.getByLabel('主模型', { exact: true }).fill('model-draft');
  await page.getByRole('button', { name: '高级配置', exact: true }).click();
  await expect(page.getByLabel('主模型', { exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: '存储位置', exact: true }).click();
  await expect(page.getByRole('button', { name: '应用数据目录', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: '语言与模型', exact: true }).click();
  await expect(page.getByLabel('主模型', { exact: true })).toHaveValue('model-draft');
  await page.getByLabel('关闭', { exact: true }).click();
  await page.evaluate(() => { (window as any).__desktopTest.host.execution = null; (window as any).__emitDesktop('workspace-changed', null); });
  await page.getByRole('button', { name: '工具环境', exact: true }).click();
  await expect(page.getByRole('button', { name: '启动环境', exact: true })).toBeVisible();
  await expect(page.getByLabel('题目名称', { exact: true })).toHaveCount(0);
  await page.getByLabel('关闭', { exact: true }).click();
  await page.getByRole('button', { name: '题目环境', exact: true }).click();
  await expect(page.getByLabel('题目名称', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '启动环境', exact: true })).toHaveCount(0);
});

test('large tool groups keep failed operations visible before expansion', async ({ page }) => {
  await page.goto('/');
  await page.evaluate(() => {
    const state = (window as any).__desktopTest;
    const original = state.data.transcript.find((item: any) => item.id === 'retry');
    const failed = original.content.find((part: any) => part.type === 'tool-call');
    state.data.transcript = [{ ...original, content: [failed, ...Array.from({ length: 6 }, (_, index) => {
      const display = { ...failed.args.display, call_id: `batch-${index}`, state: 'completed', title: 'tool-binary-inspect', summary: 'completed check' };
      return { ...failed, toolCallId: display.call_id, args: { display }, result: { display }, isError: false };
    })] }];
    state.revision++; (window as any).__emitDesktop('workspace-changed', null);
  });
  await expect(page.locator('.tool-step')).toHaveCount(1);
  await expect(page.getByText('调试后端暂不可用：连接已关闭。已保留当前证据。')).toBeVisible();
  await page.getByRole('button', { name: /展开全部/ }).click();
  await expect(page.locator('.tool-step')).toHaveCount(7);
});

test('new messages preserve reading position and count arrivals', async ({ page }) => {
  await page.goto('/');
  await page.evaluate(() => {
    const state = (window as any).__desktopTest;
    state.data.transcript = Array.from({ length: 40 }, (_, index) => ({ ...state.data.transcript[0], id: `history-${index}`, content: [{ type: 'text', text: `Progress ${index}` }] }));
    state.revision++; (window as any).__emitDesktop('workspace-changed', null);
  });
  await expect(page.getByText('Progress 39', { exact: true })).toBeVisible();
  await page.locator('.transcript').evaluate(element => { element.scrollTop = 0; });
  await expect(page.locator('.jump-latest')).toBeVisible();
  await page.evaluate(() => {
    const state = (window as any).__desktopTest;
    state.data.transcript.push({ ...state.data.transcript[0], id: 'arrival', content: [{ type: 'text', text: 'New observation' }] });
    state.revision++; (window as any).__emitDesktop('workspace-changed', null);
  });
  await expect(page.locator('.jump-latest')).toHaveText(/1/);
  expect(await page.locator('.transcript').evaluate(element => element.scrollTop)).toBeLessThan(50);
  await page.locator('.jump-latest').click();
  await expect(page.getByText('New observation', { exact: true })).toBeVisible();
});

test('bilingual layouts keep long names and maintenance controls inside the viewport', async ({ page }) => {
  await page.goto('/');
  await page.evaluate(() => {
    const state = (window as any).__desktopTest;
    state.data.runs[0].target_path = '/long/path/' + 'parser_long_name_'.repeat(12) + '.elf';
    state.revision++; (window as any).__emitDesktop('workspace-changed', null);
  });
  for (const locale of ['zh-CN', 'en-US']) {
    await page.locator('.sidebar-footer button').last().click();
    await page.locator('select').filter({ has: page.locator('option[value="en-US"]') }).first().selectOption(locale);
    await page.locator('.modal-heading button').click();
    for (const width of [1440, 1024, 860]) {
      await page.setViewportSize({ width, height: width === 1440 ? 900 : 600 });
      await expect(page.locator('.send-button')).toBeInViewport();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: `target/ui-polish-${locale}-${width}.png` });
    }
    await page.locator('.sidebar-footer button').last().click();
    await page.locator('.settings-sections button').nth(1).click();
    await expect(page.locator('.modal-footer .primary')).toBeInViewport();
    await page.screenshot({ path: `target/ui-polish-settings-${locale}.png` });
    await page.locator('.settings-sections button').first().click();
    await page.locator('.modal-heading button').click();
  }
});

test('storage edits survive closing settings without applying directory changes', async ({ page }) => {
  await page.goto('/');
  await page.evaluate(() => { const state = (window as any).__desktopTest; state.host = { ...state.host, execution: null }; (window as any).__emitDesktop('workspace-changed', null); });
  await page.getByRole('button', { name: '工作区设置', exact: true }).click();
  await page.getByRole('button', { name: '存储位置', exact: true }).click();
  await page.getByLabel('更新缓存目录', { exact: true }).fill('/new/cache/draft');
  await page.getByLabel('关闭', { exact: true }).click();
  await page.getByRole('button', { name: '工作区设置', exact: true }).click();
  await expect(page.getByLabel('更新缓存目录', { exact: true })).toHaveValue('/new/cache/draft');
  await page.getByRole('button', { name: '语言与模型', exact: true }).click();
  await page.getByRole('button', { name: '存储位置', exact: true }).click();
  await expect(page.getByLabel('更新缓存目录', { exact: true })).toHaveValue('/new/cache/draft');
  expect(await page.evaluate(() => (window as any).__desktopTest.requests.some((r: any) => r.command === 'update_cache_directory'))).toBe(false);
});

test.describe('high density desktop', () => {
  test.use({ deviceScaleFactor: 1.5, viewport: { width: 860, height: 600 } });
  test('keeps the minimum logical window usable at 150 percent pixel density', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('.send-button')).toBeInViewport();
    await page.getByRole('button', { name: '工作区设置', exact: true }).click();
    await expect(page.locator('.modal-heading button')).toBeInViewport();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: 'target/ui-polish-hidpi.png' });
  });
});

test('agent tree shows siblings, selected ancestry and collapsed subtree health', async ({ page }) => {
  await page.goto('/');
  const tree = page.getByRole('tree', { name: 'Agent 父子任务树' });
  await expect(tree.getByRole('treeitem')).toHaveCount(3);
  await expect(tree.getByRole('treeitem').nth(0)).toHaveAttribute('aria-level', '1');
  await expect(tree.getByRole('treeitem').nth(1)).toHaveAttribute('aria-level', '2');
  await expect(tree.getByRole('treeitem').nth(2)).toHaveAttribute('aria-level', '2');
  await page.locator('.agent-card').filter({ hasText: '输入边界分析' }).click();
  await expect(page.locator('.agent-breadcrumb')).toHaveText(/主分析 Agent.*输入边界分析/);
  await expect(page.locator('.agent-inspector')).toContainText('跟踪 parse_record');
  await page.getByRole('button', { name: '折叠 主分析 Agent', exact: true }).click();
  await expect(tree.getByRole('treeitem')).toHaveCount(1);
  await expect(tree).toContainText('2 个子任务');
  await expect(tree).toContainText('2 运行中');
  await expect(tree).toContainText('包含当前选中的任务');
  await page.evaluate(() => {
    const state = (window as any).__desktopTest;
    state.data.agents[2] = { ...state.data.agents[2], status: 'failed', error: 'verification backend disconnected' };
    state.revision++; (window as any).__emitDesktop('workspace-changed', null);
  });
  await expect(tree).toContainText('1 异常');
  await expect(tree.getByRole('treeitem')).toHaveCount(1);
  await page.getByRole('button', { name: '展开 主分析 Agent', exact: true }).click();
  await expect(tree.getByRole('treeitem')).toHaveCount(3);
  await expect(tree.getByText('verification backend disconnected')).toBeVisible();
  await page.screenshot({ path: 'target/agent-tree-selected.png' });
});

test('agent tree folding is scoped to the task and keyboard navigation does not select implicitly', async ({ page }) => {
  await page.goto('/');
  const tree = page.getByRole('tree');
  await tree.getByRole('treeitem').first().focus();
  await page.keyboard.press('ArrowRight');
  await expect(tree.getByRole('treeitem').nth(1)).toBeFocused();
  expect(await tree.locator('[aria-selected="true"]').count()).toBe(0);
  await page.keyboard.press('ArrowLeft');
  await expect(tree.getByRole('treeitem').first()).toBeFocused();
  await page.keyboard.press('ArrowLeft');
  await expect(tree.getByRole('treeitem')).toHaveCount(1);
  await page.locator('.run-card').filter({ hasText: 'archive.elf' }).click();
  await expect(tree).toContainText('归档分析 Agent');
  await page.locator('.run-card').filter({ hasText: 'parser.elf' }).click();
  await expect(tree.getByRole('treeitem')).toHaveCount(1);
  await page.reload();
  await expect(tree.getByRole('treeitem')).toHaveCount(1);
  await page.getByRole('button', { name: '展开 主分析 Agent', exact: true }).click();
  await expect(tree.getByRole('treeitem')).toHaveCount(3);
});

test('deep branches can be focused without inventing new parent relationships', async ({ page }) => {
  await page.goto('/');
  await page.evaluate(() => {
    const state = (window as any).__desktopTest;
    const original = state.data.agents[1];
    state.data.agents = Array.from({ length: 7 }, (_, index) => ({ ...original, id: `deep-${index}`, name: `层级 ${index}`, parent_id: index ? `deep-${index - 1}` : null, role: index ? 'analyst' : 'root' }));
    state.revision++; (window as any).__emitDesktop('workspace-changed', null);
  });
  await expect(page.getByRole('treeitem')).toHaveCount(4);
  await page.getByRole('button', { name: '聚焦分支：层级 3', exact: true }).last().click();
  await expect(page.getByRole('treeitem').first()).toContainText('层级 3');
  await expect(page.getByRole('treeitem')).toHaveCount(4);
  await page.getByRole('button', { name: '返回上级', exact: true }).click();
  await expect(page.getByRole('treeitem').first()).toContainText('层级 2');
  await page.getByRole('button', { name: '完整任务树', exact: true }).click();
  await expect(page.getByRole('treeitem').first()).toContainText('层级 0');
});

test('agent hierarchy remains readable in Chinese, English and a narrow collaboration panel', async ({ page }) => {
  await page.goto('/');
  await page.locator('.agent-card').filter({ hasText: '输入边界分析' }).click();
  await page.screenshot({ path: 'target/agent-tree-zh-CN.png' });
  await page.locator('.agents-panel').screenshot({ path: 'target/agent-tree-panel-zh-CN.png' });
  await page.getByRole('button', { name: '工作区设置', exact: true }).click();
  await page.getByLabel('界面语言', { exact: true }).selectOption('en-US');
  await page.getByLabel('Close', { exact: true }).click();
  await expect(page.locator('.agent-role').filter({ hasText: 'Analysis' }).first()).toBeVisible();
  await page.screenshot({ path: 'target/agent-tree-en-US.png' });
  await page.setViewportSize({ width: 1100, height: 700 });
  await page.getByRole('separator', { name: 'Resize collaboration panel' }).focus();
  for (let i = 0; i < 8; i++) await page.keyboard.press('ArrowRight');
  await expect(page.locator('.agents-panel')).toBeVisible();
  expect(await page.locator('.agent-list').evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.screenshot({ path: 'target/agent-tree-narrow.png' });
});

test('keyboard activation of a disclosure does not select its agent', async ({ page }) => {
  await page.goto('/');
  const collapse = page.getByRole('button', { name: '折叠 主分析 Agent', exact: true });
  await collapse.focus();
  await page.keyboard.press('Space');
  await expect(page.getByRole('treeitem')).toHaveCount(1);
  await expect(page.getByRole('treeitem').first()).toHaveAttribute('aria-selected', 'false');
  const expand = page.getByRole('button', { name: '展开 主分析 Agent', exact: true });
  await expand.focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('treeitem')).toHaveCount(3);
  await expect(page.getByRole('treeitem').first()).toHaveAttribute('aria-selected', 'false');
});


for (const locale of ['zh-CN', 'en-US']) {
  test(`first setup separates steps and errors at a small window (${locale})`, async ({ page }) => {
    await page.setViewportSize({ width: 1100, height: 700 });
    await page.goto('/');
    if (locale === 'en-US') {
      await page.getByRole('button', { name: '工作区设置', exact: true }).click();
      await page.getByLabel('界面语言').selectOption('en-US');
      await page.getByLabel('Close', { exact: true }).click();
    }
    await page.evaluate(() => {
      const native = (window as any).__TAURI_INTERNALS__, invoke = native.invoke;
      const state = { version: '0.2.0', needs_setup: true, phase: 'idle', environment_step: null,
        message: { code: 'system-current-version-is-up-to-date', args: {} }, available: null,
        notes: '', downloaded: 0, total: null, prepared: null, recovery: null, pending: false };
      Object.assign(window, { __setupTest: state });
      native.invoke = async (command: string, args: any) => {
        if (command !== 'update_action') return invoke(command, args);
        if (args.action === 'setup') {
          Object.assign(state, { phase: 'error', environment_step: 'docker', message: {
            code: 'system-docker-credential-missing', args: { helper: 'docker-credential-desktop' },
            detail: 'error getting credentials - exec: docker-credential-desktop: executable file not found in %PATH%' } });
        }
        return { ...state };
      };
    });
    await page.getByRole('button', { name: locale === 'zh-CN' ? '更新' : 'Updates', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog.locator('.setup-steps li')).toHaveCount(3);
    await expect(dialog.locator('.maintenance-options')).toHaveCount(0);
    await expect(dialog.locator('.header-actions .primary')).toBeInViewport();
    await expect(dialog.locator('.modal-heading')).toBeInViewport();
    expect(await dialog.evaluate(e => e.scrollWidth <= e.clientWidth)).toBe(true);
    await page.screenshot({ path: `target/setup-ready-${locale}.png` });
    await dialog.locator('.header-actions .primary').click();
    await expect(dialog.getByRole('alert')).toHaveCount(1);
    await expect(dialog.getByRole('alert')).toContainText('docker-credential-desktop');
    await expect(dialog.locator('.update-status')).toHaveCount(0);
    await expect(dialog.locator('.setup-steps li').first()).toHaveAttribute('data-state', 'failed');
    await dialog.locator('.update-error summary').click();
    await expect(dialog.locator('.workbench-result')).toContainText('error getting credentials');
    await dialog.locator('.header-actions .primary').scrollIntoViewIfNeeded();
    await expect(dialog.locator('.header-actions .primary')).toBeInViewport();
    await expect(dialog.locator('.modal-heading')).toBeInViewport();
    expect(await dialog.evaluate(e => e.scrollWidth <= e.clientWidth)).toBe(true);
    await page.screenshot({ path: `target/setup-error-${locale}.png` });
  });
}

test('Windows storage paths display cleanly but unchanged paths retain native identity', async ({ page }) => {
  await page.goto('/');
  const original = String.raw`\\?\C:\Users\tester\AppData\Local\WukongData\.wukong`;
  await page.evaluate(original => {
    const native = (window as any).__TAURI_INTERNALS__, invoke = native.invoke;
    const state = (window as any).__desktopTest;
    state.host = { ...state.host, execution: null, state_dir: original };
    native.invoke = async (command: string, args: any) => {
      if (command === 'update_storage') return { data_dir: original, cache_dir: String.raw`\\?\UNC\server\cache` };
      if (command === 'update_data_directory') { state.requests.push({ command, args }); return; }
      return invoke(command, args);
    };
    (window as any).__emitDesktop('workspace-changed', null);
  }, original);
  await page.getByRole('button', { name: '工作区设置', exact: true }).click();
  await page.getByRole('button', { name: '存储位置', exact: true }).click();
  await expect(page.getByLabel('任务与证据目录')).toHaveValue(original.slice(4));
  await expect(page.getByLabel('更新缓存目录')).toHaveValue(String.raw`\\server\cache`);
  page.once('dialog', dialog => dialog.accept());
  await page.getByRole('button', { name: '应用数据目录' }).click();
  const requests = await page.evaluate(() => (window as any).__desktopTest.requests);
  expect(requests.find((r: any) => r.command === 'update_data_directory')?.args.destination).toBe(original);
});


test('image layer progress updates while pulling and keeps self-check separate', async ({ page }) => {
  await page.setViewportSize({ width: 1100, height: 700 });
  await page.goto('/');
  await page.evaluate(() => {
    const native = (window as any).__TAURI_INTERNALS__, invoke = native.invoke;
    const progress = { layers: [{ id: 'abcdef123456', state: 'ready' }, { id: '123456abcdef', state: 'downloaded' }], elapsed_ms: 65000, finished: false, cached: false, truncated: false };
    Object.assign(window, { __pullTest: progress });
    native.invoke = async (command: string, args: any) => command === 'update_action' ? {
      version: '0.2.0', needs_setup: true, phase: progress.finished ? 'validating' : 'pulling', environment_step: progress.finished ? 'validating' : 'pulling',
      operation_id: 'pull-progress', revision: progress.finished ? 101 : 100, busy: true, cancellable: true, action: 'setup',
      image_version: '0.2.0', image_progress: progress,
      message: { code: progress.finished ? 'system-checking-the-candidate-tool-environment' : 'system-pulling-the-tool-image-docker-will-reuse-downloaded', args: {} },
      available: null, notes: '', downloaded: 0, total: null, prepared: null, recovery: null, pending: false,
    } : invoke(command, args);
  });
  await page.getByRole('button', { name: '更新', exact: true }).click();
  await expect(page.getByText('已就绪 1 层 / 已发现 2 层')).toBeVisible();
  await page.getByText('镜像层详情', { exact: true }).click();
  await expect(page.locator('.pull-layers')).toContainText('已下载');
  await page.locator('.image-pull-progress').scrollIntoViewIfNeeded();
  await expect(page.locator('.modal-heading')).toBeInViewport();
  await page.screenshot({ path: 'target/setup-image-progress.png' });
  await page.evaluate(() => { const p = (window as any).__pullTest; p.finished = true; p.layers[1].state = 'ready'; });
  await expect(page.getByText('镜像已拉取，环境自检仍需单独完成')).toBeVisible();
  await expect(page.locator('.setup-steps li').nth(2)).toHaveAttribute('data-state', 'active');
  await expect(page.getByRole('progressbar')).toHaveCount(0);
});


test('background preparation reattaches from either entry and cancellation settles without restarting', async ({ page }) => {
  await page.goto('/');
  await page.evaluate(() => {
    const native = (window as any).__TAURI_INTERNALS__, invoke = native.invoke;
    const state = { version: '0.2.0', needs_setup: true, phase: 'pulling', environment_step: 'pulling',
      operation_id: 'same-operation', revision: 100, busy: true, cancellable: true, cancelling: false,
      message: { code: 'system-pulling-the-tool-image-docker-will-reuse-downloaded', args: {} },
      available: null, notes: '', downloaded: 0, total: null, prepared: null, recovery: null, pending: false };
    Object.assign(window, { __backgroundState: state, __cancelIds: [] });
    native.invoke = async (command: string, args: any) => {
      if (command === 'update_action') return { ...state };
      if (command === 'update_cancel') {
        (window as any).__cancelIds.push(args.operationId);
        Object.assign(state, { revision: 101, busy: false, cancellable: false, phase: 'cancelled', message: { code: 'system-preparation-cancelled', args: {} } });
        return { ...state };
      }
      return invoke(command, args);
    };
  });
  await page.getByRole('button', { name: '更新', exact: true }).click();
  await page.getByRole('button', { name: '收起到后台', exact: true }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.locator('.sidebar-operation')).toContainText('正在拉取');
  await page.getByRole('button', { name: '工具环境', exact: true }).click();
  await expect(page.getByRole('button', { name: '自检并修复环境' })).toBeDisabled();
  await page.getByRole('button', { name: '取消准备', exact: true }).click();
  await expect(page.getByRole('dialog')).toContainText('准备已取消');
  await expect(page.getByRole('button', { name: '自检并修复环境' })).toBeEnabled();
  expect(await page.evaluate(() => (window as any).__cancelIds)).toEqual(['same-operation']);
});


test('readable typography survives large displays and the minimum workspace', async ({ page }) => {
  await page.goto('/');
  for (const [width, height] of [[2560, 1369], [1100, 700]]) {
    await page.setViewportSize({ width, height });
    await expect(page.locator('.composer textarea')).toBeVisible();
    expect(await page.locator('.composer textarea').evaluate(e => parseFloat(getComputedStyle(e).fontSize))).toBeGreaterThanOrEqual(16);
    expect(await page.locator('.markdown').first().evaluate(e => parseFloat(getComputedStyle(e).fontSize))).toBeGreaterThanOrEqual(16);
    expect(await page.getByRole('button', { name: '工作区设置', exact: true }).evaluate(e => parseFloat(getComputedStyle(e).fontSize))).toBeGreaterThanOrEqual(14);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: `target/typography-workspace-${width}.png` });
  }
  await page.setViewportSize({ width: 2560, height: 1369 });
  await page.evaluate(() => {
    const state = (window as any).__desktopTest;
    state.data.runs = []; state.data.selected = null; state.host.execution = null; state.revision++;
    (window as any).__emitDesktop('workspace-changed', null);
  });
  await expect(page.locator('.welcome')).toBeVisible();
  await page.screenshot({ path: 'target/typography-welcome-2560.png' });
});
