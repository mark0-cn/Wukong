import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { vi, it, expect } from 'vitest';
import { Composer, type Draft } from '../components/Conversation';
import { agents } from './fixtures';
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
