import { test, expect, type Page } from '@playwright/test';
import { host, workspace } from '../src/shared/testing/fixtures';

const now = Date.now();
const stamp = (offset = 0) => new Date(now + offset).toISOString();
const traceId = '24b5ec2b2ea7de034bc281f8f3d1e5ba';
const signal = (data: unknown, overrides: Record<string, unknown> = {}) => ({
  configured: true, partial: false, truncated: false, stale: false,
  queried_at: stamp(), error: null, data, ...overrides,
});
const local = {
  activity_kind: 'local_business_activity',
  run: { id: 'run-a', status: 'running', root_id: 'root', created_at: stamp(-60000), updated_at: stamp(),
    total_tokens: 42, cost_usd: 0.25, summary: 'Persistent browser acceptance activity' },
  agents: [{ id: 'root', parent_id: null, name: 'Lead', role: 'root', status: 'running', generation: 1,
    turns: 2, total_tokens: 42, cost_usd: 0.25, created_at: stamp(-60000), updated_at: stamp() }],
  events: [{ seq: 1, agent_id: 'root', kind: 'message', created_at: stamp(), data: { text: 'Durable event remains available' }, data_truncated: false }],
  tools: { total: 1, prepared: 0, running: 1, completed: 0, failed: 0, uncertain: 0, side_effecting: 1 },
  resources: { jobs: 1, active_jobs: 1, terminal_jobs: 0, evidence_pending: 0 },
  next_cursor: null, has_more: false, partial: false, truncated: false, stale: false, queried_at: stamp(),
};
const traces = { run_id: 'run-a', next_cursor: null, traces: [{ trace_id: traceId,
  root_service_name: 'wukong', root_trace_name: 'wukong.assessment', start_time: stamp(-30000), duration_ms: 250, span_count: 2 }] };
const detail = { trace_id: traceId, spans: [
  { trace_id: traceId, span_id: '00f067aa0ba902b7', parent_span_id: null, name: 'wukong.assessment',
    start_time: stamp(-30000), end_time: stamp(-20000), duration_nanos: 10000000000, status: 'ok',
    attributes: { run_id: 'run-a' }, resource: { 'service.name': 'wukong' } },
  { trace_id: traceId, span_id: '00f067aa0ba902b8', parent_span_id: '00f067aa0ba902b7', name: 'execute_tool',
    start_time: stamp(-25000), end_time: stamp(-22000), duration_nanos: 3000000000, status: 'ok',
    attributes: { 'gen_ai.tool.name': 'binary_info' }, resource: { 'service.name': 'wukong' } },
] };
const logs = { run_id: 'run-a', trace_id: null, next_cursor: null, entries: [{ timestamp: stamp(), timestamp_nanos: String(now * 1000000),
  line: 'run_finished outcome=completed', line_truncated: false,
  labels: { service_name: 'wukong', level: 'info', target: 'wukong_telemetry' }, attributes: { run_id: 'run-a' } }] };
const metrics = { metric: 'runs_started', expression: 'sum(wukong_run_started_total)', scope: 'service', step_seconds: 30,
  series: [{ labels: { service_name: 'wukong' }, points: Array.from({ length: 12 }, (_, index) => ({ timestamp: stamp((index - 11) * 5000), value: index + 1 })) }] };

async function mockDesktop(page: Page, degraded = false) {
  await page.addInitScript(({ data, host, local, traces, detail, logs, metrics, traceId, degraded }) => {
    const state = { data, host, preferences: { ui_language: 'en-US', output_language: 'auto', system_locale: 'en-US' } };
    const signal = (data: unknown, overrides: Record<string, unknown> = {}) => ({
      configured: true, partial: false, truncated: false, stale: false,
      queried_at: new Date().toISOString(), error: null, data, ...overrides,
    });
    const callbacks = new Map<number, (value: unknown) => void>();
    const listeners = new Map<string, number[]>();
    let next = 0;
    Object.assign(window, {
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} },
      __TAURI_INTERNALS__: {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: (callback: (value: unknown) => void) => { callbacks.set(++next, callback); return next; },
        unregisterCallback: (id: number) => callbacks.delete(id),
        invoke: async (command: string, args: Record<string, any> = {}) => {
          if (command === 'preferences_read' || command === 'preferences_save') return state.preferences;
          if (command === 'update_action') return { version: '1.0.0', phase: 'idle', message: '', available: null,
            notes: null, downloaded: 0, total: null, prepared: null, recovery: null, needs_setup: false, pending: false };
          if (command === 'update_storage') return { data_dir: '/data/workspace', cache_dir: '/data/cache' };
          if (command === 'workbench_action') return { state: 'ready' };
          if (command === 'plugin:event|listen') { listeners.set(args.event, [...(listeners.get(args.event) ?? []), args.handler]); return args.handler; }
          if (command === 'plugin:event|unlisten') return;
          if (command === 'workspace_read') return { host: structuredClone(state.host), workspace: { revision: 'browser-fixture', data: structuredClone(state.data) } };
          if (command === 'observability_status') return {
            capabilities: Object.fromEntries(['tempo', 'loki', 'prometheus', 'grafana'].map(name => [name,
              { configured: !degraded || name === 'tempo', origin: `http://127.0.0.1/${name}`, credential_available: false }])),
            exporter: { activated: true, traces: true, metrics: true, logs: true, local_jsonl: true, worker_relay: true,
              worker_relay_status: { traces: { healthy: true, last_error: null }, logs: { healthy: true, last_error: null } }, last_error: null },
            queried_at: new Date().toISOString(),
          };
          if (command === 'observability_run') return { local,
            traces: signal(traces),
            logs: degraded ? signal(null, { partial: true, error: { kind: 'unavailable', message: 'Loki is unavailable', status: 503 } }) : signal(logs),
            metrics: degraded ? signal(null, { configured: false }) : signal(metrics) };
          if (command === 'observability_trace') return signal(detail);
          if (command === 'observability_grafana_link') return `http://127.0.0.1:3000/explore?trace=${traceId}`;
          throw new Error(`Unhandled browser fixture command: ${command}`);
        },
      },
    });
  }, { data: workspace, host, local, traces, detail, logs, metrics, traceId, degraded });
}

test('renders real chart and timeline libraries with correlated run data', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await mockDesktop(page);
  await page.goto('/');
  await page.getByRole('button', { name: 'Observability', exact: true }).click();
  await expect(page.getByText('Persistent browser acceptance activity')).toBeVisible();
  await page.getByRole('button', { name: 'Traces', exact: true }).click();
  await page.getByRole('button', { name: /wukong\.assessment/ }).click();
  await expect(page.locator('.observability-timeline .vis-timeline')).toBeVisible();
  await expect(page.locator('.observability-timeline .vis-item')).toHaveCount(2);
  await expect(page.locator('.observability-span-details')).toContainText('run-a');
  await page.locator('.observability-timeline').scrollIntoViewIfNeeded();
  await page.screenshot({ path: 'test-results/observability-trace.png', fullPage: true });
  await page.getByRole('button', { name: 'Metrics', exact: true }).click();
  await expect(page.locator('.observability-chart canvas')).toBeVisible();
  expect(await page.locator('.observability-chart canvas').evaluate(canvas => (canvas as HTMLCanvasElement).width)).toBeGreaterThan(0);
  await page.locator('.observability-chart').scrollIntoViewIfNeeded();
  await page.screenshot({ path: 'test-results/observability-desktop.png', fullPage: true });
  expect(errors).toEqual([]);
});

test('keeps durable activity and trace usable during independent backend degradation', async ({ page }) => {
  await mockDesktop(page, true);
  await page.goto('/');
  await page.getByRole('button', { name: 'Observability', exact: true }).click();
  await expect(page.getByText('Persistent browser acceptance activity')).toBeVisible();
  await page.getByRole('button', { name: 'Logs', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Loki is unavailable');
  await page.getByRole('button', { name: 'Traces', exact: true }).click();
  await expect(page.getByText('wukong.assessment', { exact: true })).toBeVisible();
  await page.emulateMedia({ reducedMotion: 'reduce', colorScheme: 'dark' });
  await page.setViewportSize({ width: 860, height: 600 });
  await page.getByText('wukong.assessment', { exact: true }).scrollIntoViewIfNeeded();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: 'test-results/observability-degraded-narrow.png', fullPage: true });
});
