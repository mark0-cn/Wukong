import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { vi, it, expect } from 'vitest';
import { Composer, type Draft } from './Conversation';
import { agents } from '../../shared/testing/fixtures';
it('binds delivery to the displayed generation and keeps text typed while sending', async () => {
  let finish!: () => void;
  const send = vi.fn(() => new Promise<void>(resolve => { finish = resolve; }));
  function Harness() {
    const [draft, onDraft] = useState<Draft>({ text: 'first', followUp: false });
    return <Composer target={agents[1]} draft={draft} onDraft={onDraft} send={send} busy={false} />;
  }
  render(<Harness />);
  fireEvent.click(screen.getByLabelText('发送消息'));
  await waitFor(() => expect(send).toHaveBeenCalledOnce());
  expect(send.mock.calls[0]).toEqual([{ text: 'first', followUp: false }, agents[1]]);
  fireEvent.change(screen.getByLabelText('发送给 Agent 的消息'), { target: { value: 'newer text' } });
  finish();
  await waitFor(() => expect((screen.getByLabelText('发送给 Agent 的消息') as HTMLTextAreaElement).value).toBe('newer text'));
});

it('requires an explicit follow-up for a completed agent', async () => {
  const send = vi.fn().mockResolvedValue(undefined);
  const target = { ...agents[1], status: 'completed' as const };
  function Harness() {
    const [draft, onDraft] = useState<Draft>({ text: 'continue investigation', followUp: false });
    return <Composer target={target} draft={draft} onDraft={onDraft} send={send} busy={false} />;
  }
  render(<Harness />);
  fireEvent.click(screen.getByLabelText('发送消息'));
  expect(send).not.toHaveBeenCalled();
  fireEvent.change(screen.getByLabelText('发送方式'), { target: { value: 'follow_up' } });
  fireEvent.click(screen.getByLabelText('发送消息'));
  await waitFor(() => expect(send).toHaveBeenCalledWith({ text: 'continue investigation', followUp: true }, target));
});

it('ignores composing Ctrl+Enter and disables input without a recipient', () => {
  const send = vi.fn().mockResolvedValue(undefined);
  const draft = { text: 'keep composing', followUp: false };
  const { rerender } = render(<Composer target={agents[1]} draft={draft} onDraft={vi.fn()} send={send} busy={false} />);
  const composer = screen.getByLabelText('发送给 Agent 的消息');
  fireEvent.keyDown(composer, { key: 'Enter', ctrlKey: true, isComposing: true });
  expect(send).not.toHaveBeenCalled();
  rerender(<Composer draft={draft} onDraft={vi.fn()} send={send} busy={false} />);
  expect((composer as HTMLTextAreaElement).disabled).toBe(true);
  expect((screen.getByLabelText('发送消息') as HTMLButtonElement).disabled).toBe(true);
});
