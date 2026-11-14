import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, test, vi } from 'vitest';
import { useState } from 'react';
import { WindowsConnection, emptyWindowsConnection } from './WindowsConnection';

function Connection({ onAdd, hasEnvironments = false }: { onAdd: (input: unknown) => Promise<void>; hasEnvironments?: boolean }) {
  const [draft, onDraft] = useState(emptyWindowsConnection);
  return <WindowsConnection disabled={false} hasEnvironments={hasEnvironments} draft={draft} onDraft={onDraft} onAdd={onAdd} />;
}

test('a connection draft requires an independently entered matching fingerprint and keeps the key as a path', async () => {
  const onAdd = vi.fn().mockResolvedValue(undefined);
  render(<Connection onAdd={onAdd} />);
  const add = screen.getByRole('button', { name: '加入设置草稿' }) as HTMLButtonElement;
  expect(add.disabled).toBe(true);
  const receipt = JSON.stringify({ host_fingerprint: 'SHA256:public-fixture' });
  fireEvent.change(screen.getByLabelText('公开连接 JSON'), { target: { value: receipt } });
  fireEvent.change(screen.getByLabelText('虚拟机地址（IP 或主机名）'), { target: { value: '192.0.2.1' } });
  fireEvent.change(screen.getByLabelText('控制端 SSH 私钥文件'), { target: { value: '/controller/existing-key' } });
  expect(add.disabled).toBe(true);
  fireEvent.change(screen.getByLabelText('虚拟机界面显示的主机指纹'), { target: { value: 'SHA256:wrong' } });
  expect(screen.getByRole('alert').textContent).toContain('指纹不一致');
  expect(add.disabled).toBe(true);
  fireEvent.change(screen.getByLabelText('虚拟机界面显示的主机指纹'), { target: { value: 'SHA256:public-fixture' } });
  fireEvent.click(add);
  await waitFor(() => expect(onAdd).toHaveBeenCalledOnce());
  expect(onAdd).toHaveBeenCalledWith({ connection_json: receipt, name: 'Windows 虚拟机', host: '192.0.2.1',
    identity_file: '/controller/existing-key', confirmed_fingerprint: 'SHA256:public-fixture', make_default: true });
  await waitFor(() => expect(screen.getByRole('status').textContent).toContain('连接尚未验证'));
});

test('failed registration preserves the editable connection draft', async () => {
  const onAdd = vi.fn().mockRejectedValue(new Error('Environment ID already exists'));
  render(<Connection hasEnvironments onAdd={onAdd} />);
  fireEvent.change(screen.getByLabelText('公开连接 JSON'), { target: { value: '{"host_fingerprint":"SHA256:fixture"}' } });
  fireEvent.change(screen.getByLabelText('虚拟机地址（IP 或主机名）'), { target: { value: '192.0.2.7' } });
  fireEvent.change(screen.getByLabelText('控制端 SSH 私钥文件'), { target: { value: '/controller/key' } });
  fireEvent.change(screen.getByLabelText('虚拟机界面显示的主机指纹'), { target: { value: 'SHA256:fixture' } });
  fireEvent.click(screen.getByRole('button', { name: '加入设置草稿' }));
  await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('Environment ID already exists'));
  expect((screen.getByLabelText('虚拟机地址（IP 或主机名）') as HTMLInputElement).value).toBe('192.0.2.7');
  expect((screen.getByLabelText('虚拟机界面显示的主机指纹') as HTMLInputElement).value).toBe('SHA256:fixture');
  expect((screen.getByLabelText('用于新的分析任务') as HTMLInputElement).checked).toBe(false);
});
