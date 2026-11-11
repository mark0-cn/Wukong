import { act, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { convertMessage, ToolStep } from '../components/Conversation';
import { api } from '../bridge';
import { tool, workspace } from './fixtures';
import { ToolDuration } from '../components/ToolActivity';

vi.mock('../bridge', () => ({ api: vi.fn(), errorText: String, errorValue: String, openLink: vi.fn() }));

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
        started_at: '2026-11-11T09:59:55Z', finished_at: null, observed_at: '2026-11-11T10:00:00Z', attempts: null, successes: null } };
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
    render(<ToolStep display={{ ...tool, preview: { text: code, language: 'c', evidence_id: 'raw-ghidra-evidence', truncated: true } }} />);
    expect(screen.getByText(code)).not.toBeNull();
    expect(screen.getByText('raw-ghidra-evidence')).not.toBeNull();
    expect(screen.getByText(/预览已截断|Preview truncated/)).not.toBeNull();
    expect(document.querySelector('script')).toBeNull();
    expect(api).not.toHaveBeenCalled();
  });
});
