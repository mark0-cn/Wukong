import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { convertMessage, ToolStep } from './Conversation';
import { api } from '../../shared/api/bridge';
import { tool, workspace } from '../../shared/testing/fixtures';
import { ToolDuration } from './ToolActivity';
import type { ToolDisplay, ToolRecord } from '../../shared/api/types';

vi.mock('../../shared/api/bridge', () => ({ api: vi.fn(), errorText: String, errorValue: String, openLink: vi.fn() }));

function queryClient() {
  return new QueryClient({ defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false, refetchOnReconnect: false } } });
}
function withQuery(client: QueryClient, children: ReactNode) {
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}
beforeEach(() => { vi.mocked(api).mockReset(); });

function toolRecord(id: string, marker: string, state = 'completed'): ToolRecord {
  return { id, agent_id: 'agent', name: 'tool', arguments: { marker }, result: null, evidence_id: null, state, created_at: '', updated_at: '' };
}

function pendingCleanupDisplay(): ToolDisplay {
  return {
    ...tool, call_id: 'call-cleanup', title: 'tool-job-status', state: 'completed',
    summary: '后台任务已完成', evidence_id: null,
    job: {
      id: 'job-cleanup', state: 'completed', stale: false,
      started_at: '2026-11-11T09:59:55Z', finished_at: '2026-11-11T10:00:01Z',
      observed_at: '2026-11-11T10:00:02Z', attempts: null, successes: null,
      execution: { finished: true, outcome: 'known', exit_code: 0, passed: true, evidence_id: 'execution-evidence' },
      cleanup: { status: 'pending', evidence_id: null },
      blocking: { reason: { code: 'progress-job-blocked-cleanup', args: {} }, detail: 'cleanup reconciliation is required' },
      evidence: [{ role: 'execution', evidence_id: 'execution-evidence' }],
    },
  };
}

describe('evidence-backed tool presentation', () => {
  it('preserves identity while partial narration changes status', () => {
    const message = { ...workspace.transcript[0], id: 'reply-stable', state: 'streaming' as const };
    expect(convertMessage(message).status).toEqual({ type: 'running' });
    const complete = convertMessage({ ...message, state: 'complete' });
    expect(complete.id).toBe(message.id);
    expect(complete.status).toEqual({ type: 'complete', reason: 'stop' });
    expect(convertMessage({ ...message, state: 'interrupted' }).status).toEqual({ type: 'incomplete', reason: 'cancelled' });
  });
  it('keeps background time live and freezes it on the recorded completion', () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date('2026-11-11T10:00:00Z'));
      const display = { ...tool, state: 'running', duration_ms: null, job: { id: 'job', state: 'running', stale: false,
        started_at: '2026-11-11T09:59:55Z', finished_at: null, observed_at: '2026-11-11T10:00:00Z', attempts: null, successes: null,
        execution: { finished: false, outcome: 'known' as const, exit_code: null, passed: null, evidence_id: null },
        cleanup: { status: 'unconfirmed' as const, evidence_id: null }, evidence: [] } };
      const view = render(<ToolDuration display={display} />);
      expect(screen.getByText('5.0s')).not.toBeNull();
      act(() => vi.advanceTimersByTime(2000));
      expect(screen.getByText('7.0s')).not.toBeNull();
      view.rerender(<ToolDuration display={{ ...display, state: 'completed', duration_ms: 6500 }} />);
      act(() => vi.advanceTimersByTime(5000));
      expect(screen.getByText('6.5s')).not.toBeNull();
      view.unmount();
    } finally { vi.useRealTimers(); }
  });
  it('renders literal code and provenance without fetching raw details eagerly', () => {
    const code = 'char *text = "<script>not executable</script>";';
    render(withQuery(queryClient(), <ToolStep display={{ ...tool, preview: { text: code, language: 'c', evidence_id: 'raw-ghidra-evidence', truncated: true } }} />));
    expect(screen.getByText(code)).not.toBeNull();
    expect(screen.getByText('raw-ghidra-evidence')).not.toBeNull();
    expect(screen.getByText(/预览已截断|Preview truncated/)).not.toBeNull();
    expect(document.querySelector('script')).toBeNull();
    expect(api).not.toHaveBeenCalled();
  });
  it('shows a finished execution, pending cleanup and the blocking reason as separate facts', () => {
    const { container } = render(withQuery(queryClient(), <ToolStep display={pendingCleanupDisplay()} />));
    expect(container.querySelector('.job-execution')!.textContent).toContain('已结束');
    expect(container.querySelector('.job-cleanup')!.textContent).toContain('等待确认');
    const blocking = container.querySelector('.job-blocking')!;
    expect(blocking.getAttribute('role')).toBe('status');
    expect(blocking.textContent).toContain('Worker 尚未确认其资源已清理。');
    expect(blocking.textContent).toContain('cleanup reconciliation is required');
    expect(container.querySelector('.job-evidence')!.textContent).toContain('execution-evidence');
    expect(container.querySelector('.job-cleanup')!.textContent).not.toContain('已确认');
  });
  it('keeps the pending cleanup when a refreshed snapshot replaces the row', () => {
    const first = pendingCleanupDisplay();
    const client = queryClient();
    const view = render(withQuery(client, <ToolStep display={first} />));
    // The workspace poll returns the same persisted projection as a new object.
    view.rerender(withQuery(client, <ToolStep display={JSON.parse(JSON.stringify(first)) as ToolDisplay} />));
    expect(view.container.querySelector('.job-cleanup')!.textContent).toContain('等待确认');
    expect(view.container.querySelector('.job-blocking')).not.toBeNull();
    // Only a snapshot with confirmed cleanup clears the blocker.
    const confirmed: ToolDisplay = { ...first, job: { ...first.job!, cleanup: { status: 'confirmed', evidence_id: 'cleanup-evidence' }, blocking: undefined } };
    view.rerender(withQuery(client, <ToolStep display={confirmed} />));
    expect(view.container.querySelector('.job-cleanup')!.textContent).toContain('已确认');
    expect(view.container.querySelector('.job-blocking')).toBeNull();
  });
  it('does not retry failed tool detail reads', async () => {
    vi.mocked(api<'tool_details'>).mockRejectedValue(new Error('tool detail failed'));
    const { container } = render(withQuery(queryClient(), <ToolStep display={{ ...tool, state: 'completed' }} />));
    fireEvent.click(container.querySelector('.tool-trigger')!);
    await screen.findByRole('alert');
    expect(api).toHaveBeenCalledTimes(1);
  });
  it('clears old details on state changes and ignores late responses from another call', async () => {
    let resolveOld!: (value: ToolRecord) => void;
    vi.mocked(api<'tool_details'>).mockImplementation((_command, args) => {
      const value = toolRecord(args!.id, args?.id === 'old' ? 'old-call' : 'new-call');
      return args?.id === 'old' ? new Promise(resolve => { resolveOld = resolve; }) : Promise.resolve(value);
    });
    const client = queryClient();
    const view = render(withQuery(client, <ToolStep display={{ ...tool, call_id: 'old', state: 'completed' }} />));
    fireEvent.click(view.container.querySelector('.tool-trigger')!);
    await waitFor(() => expect(api).toHaveBeenCalledTimes(1));
    view.rerender(withQuery(client, <ToolStep display={{ ...tool, call_id: 'new', state: 'completed' }} />));
    await waitFor(() => expect(view.container.querySelector('.tool-details')!.textContent).toContain('new-call'));
    await act(async () => resolveOld(toolRecord('old', 'old-call')));
    expect(view.container.querySelector('.tool-details')!.textContent).not.toContain('old-call');
    expect(view.container.querySelector('.tool-details')!.textContent).toContain('new-call');

    let resolveState!: (value: ToolRecord) => void;
    vi.mocked(api<'tool_details'>).mockImplementationOnce(() => new Promise(resolve => { resolveState = resolve; }));
    view.rerender(withQuery(client, <ToolStep display={{ ...tool, call_id: 'new', state: 'failed' }} />));
    expect(view.container.querySelector('.tool-details')!.textContent).not.toContain('new-call');
    await act(async () => resolveState(toolRecord('new', 'failed-state', 'failed')));
    await waitFor(() => expect(view.container.querySelector('.tool-details')!.textContent).toContain('failed-state'));
  });
});
