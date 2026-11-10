import { test, expect, type Page } from '@playwright/test';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { agents, budget, finding, host, tool, workspace } from '../src/test/fixtures';

async function mockDesktop(page: Page) {
  await page.addInitScript(({ data, host, tool, finding }) => {
    const state = { data, host, preferences: { ui_language: 'zh-CN', output_language: 'auto', system_locale: 'en-US' }, requests: [] as { command: string; args: Record<string, any> }[], revision: 1, sendDelay: 0 };
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
        if (command === 'update_action') return { version: '0.2.0', phase: 'idle', message: '当前工具环境已就绪', available: '0.2.1', notes: '修复工具兼容性', downloaded: 0, total: null, prepared: null, recovery: null, needs_setup: false, pending: false };
        if (command === 'plugin:event|listen') { listeners.set(args.event, [...(listeners.get(args.event) ?? []), args.handler]); return args.handler; }
        if (command === 'plugin:event|unlisten') { listeners.set(args.event, (listeners.get(args.event) ?? []).filter(id => id !== args.eventId)); return; }
        if (command === 'workspace_read') {
          const request = args.request;
          const snapshot = structuredClone(state.data);
          snapshot.selected = request.run ?? snapshot.selected;
          if (snapshot.selected === 'run-b') { snapshot.agents = [{ ...snapshot.agents[0], id: 'root-b', name: '归档分析 Agent' }]; snapshot.transcript = []; snapshot.active_tools = []; }
          else if (request.agent) snapshot.transcript = snapshot.transcript.filter(message => message.agent_id === request.agent);
          const revision = `${state.revision}:${snapshot.selected}:${request.agent ?? ''}:${request.event_limit}`;
          return { host: state.host, workspace: { revision, data: revision === request.known_revision ? null : snapshot } };
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

test('language changes preserve active work, drafts and configuration identity', async ({ page }) => {
  await page.goto('/');
  await page.getByLabel('发送给 Agent 的消息').fill('keep this exact payload: 原始字节');
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
  await page.setViewportSize({ width: 1024, height: 680 });
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
  await page.getByRole('button', { name: 'reproduction.txt', exact: true }).click();
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
  await page.getByRole('button', { name: '更新与环境', exact: true }).click();
  await expect(page.getByText('0.2.0', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '安装并重启', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: '下载并验证更新', exact: true })).toBeEnabled();
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
  await expect(page.getByRole('button', { name: '更新与环境', exact: true })).toBeInViewport();
  await page.getByRole('button', { name: '发现与证据', exact: false }).click();
  await page.getByRole('button', { name: 'reproduction.txt', exact: true }).click();
  await expect(page.getByText('first evidence page: controlled reproduction')).toBeVisible();
  await page.getByLabel('下一页证据').scrollIntoViewIfNeeded();
  await expect(page.getByLabel('下一页证据')).toBeInViewport();
  await page.getByLabel('下一页证据').click();
  await expect(page.getByText('second evidence page: end of output')).toBeVisible();
  await page.screenshot({ path: join(tmpdir(), 'wukong-workspace-narrow.png'), fullPage: true });
});
