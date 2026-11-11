import { render, screen, fireEvent } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { CopyButton } from '../components/CopyButton';

it('preserves evidence bytes when copying and reports clipboard failure', async () => {
  const writeText = vi.fn().mockRejectedValueOnce(new Error('clipboard unavailable')).mockResolvedValueOnce(undefined);
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
  render(<CopyButton value={'0x0041\n原始字节'} label="复制证据" />);
  fireEvent.click(screen.getByRole('button', { name: '复制证据' }));
  await screen.findByText('复制失败，请手动选择文本复制。');
  fireEvent.click(screen.getByRole('button', { name: '复制证据' }));
  await screen.findByText('已复制');
  expect(writeText).toHaveBeenLastCalledWith('0x0041\n原始字节');
});
