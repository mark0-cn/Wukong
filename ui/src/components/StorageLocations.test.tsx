import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import { api } from '../bridge';
import { StorageLocations } from './StorageLocations';
vi.mock('../bridge', () => ({ api: vi.fn(), errorText: (e: unknown) => String(e) }));
afterEach(() => { vi.restoreAllMocks(); vi.mocked(api).mockReset(); });
test('data directory switch requires confirmation and preserves cancelled choice', async () => {
  vi.mocked(api).mockResolvedValue({ data_dir: 'C:/data', cache_dir: 'C:/cache' });
  vi.spyOn(window, 'confirm').mockReturnValue(false);
  render(<StorageLocations epoch={3} disabled={false} onChanged={() => {}} />);
  await screen.findByDisplayValue('C:/data');
  await userEvent.click(screen.getByRole('button', { name: '应用数据目录' }));
  expect(vi.mocked(api).mock.calls.some(([command]) => command === 'update_data_directory')).toBe(false);
});
test('cache folder picker and save use the current workspace epoch', async () => {
  vi.mocked(api).mockImplementation(async (command) => command === 'update_storage' ? { data_dir: '/data', cache_dir: '/cache' } : command === 'pick_directory' ? '/new-cache' : '/new-cache');
  const changed = vi.fn();
  render(<StorageLocations epoch={7} disabled={false} onChanged={changed} />);
  await screen.findByDisplayValue('/cache');
  await userEvent.click(screen.getByRole('button', { name: '选择缓存目录' }));
  await screen.findByDisplayValue('/new-cache');
  await userEvent.click(screen.getByRole('button', { name: '应用缓存目录' }));
  expect(api).toHaveBeenCalledWith('update_cache_directory', { epoch: 7, destination: '/new-cache' });
  expect(changed).toHaveBeenCalled();
});
