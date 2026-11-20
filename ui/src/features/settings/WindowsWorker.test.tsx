import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { useState } from 'react';
import { WindowsWorker } from './WindowsWorker';
import { api } from '../../shared/api/bridge';
import type {
  WorkerMediaGeneration, WorkerMediaImportResult, WorkerMediaInstallation,
  WorkerMediaNetwork, WorkerMediaStatus,
} from '../../shared/api/commands';

vi.mock('../../shared/api/bridge', () => ({
  api: vi.fn(),
  errorText: (error: unknown) => typeof error === 'string' ? error
    : error && typeof error === 'object' && 'detail' in error ? String((error as { detail: unknown }).detail)
      : error instanceof Error ? error.message : String(error),
  errorValue: (error: unknown) => error,
}));

afterEach(() => { vi.mocked(api).mockReset(); });

type Handlers = Record<string, (args?: any) => unknown>;
function useApi(handlers: Handlers) {
  vi.mocked(api).mockImplementation((((command: string, args?: unknown) => {
    const handler = handlers[command];
    if (!handler) return Promise.reject(new Error(`unexpected command ${command}`));
    try { return Promise.resolve(handler(args)); } catch (error) { return Promise.reject(error); }
  }) as unknown as typeof api));
}

const INSTALLATION_ID = '0123456789abcdef0123456789abcdef';
const environmentId = 'windows-01234567';

function diagnostics(): WorkerMediaInstallation['diagnostics'] {
  return {
    generation_record: 'present', recovery_record: 'missing',
    receipt: 'missing', registration: 'missing', verification: 'not_started',
  };
}

function installation(overrides: Partial<WorkerMediaInstallation> = {}): WorkerMediaInstallation {
  return {
    installation_id: INSTALLATION_ID, environment_id: environmentId,
    environment_name: 'Windows Worker 01234567', state: 'generated',
    app_version: '1.0.0', ssh_port: 22240,
    management_peer: '192.168.199.1', network_prefix_len: 24,
    zip_path: 'C:\\out\\worker.zip', zip_present: true,
    media_path: 'C:\\state\\media', media_present: true,
    created_at: '2026-11-18T00:00:00Z',
    registered_environment_id: null, registered_environment_name: null,
    last_address: null, candidate_addresses: [], warning: null,
    diagnostics: diagnostics(),
    ...overrides,
  };
}

function network(overrides: Partial<WorkerMediaNetwork> = {}): WorkerMediaNetwork {
  return {
    id: 'vmnet8', name: 'VMnet8', description: 'VMware host-only adapter',
    ipv4: '192.168.199.1', prefix_len: 24, rfc1918: true,
    virtual_adapter: true, recommended: true, ...overrides,
  };
}

const status = (installations: WorkerMediaInstallation[] = [],
  diagnostics: WorkerMediaStatus['diagnostics'] = { inventory: 'present', unreadable_generation_records: [] }): WorkerMediaStatus =>
  ({ environments: [], installations, diagnostics });

function generation(): WorkerMediaGeneration {
  return {
    installation_id: INSTALLATION_ID, environment_id: environmentId,
    environment_name: 'Windows Worker 01234567', installation_state: 'generated',
    app_version: '1.0.0', zip_path: 'C:\\out\\worker.zip',
    zip_sha256: 'a'.repeat(64), zip_size: 1024, zip_entries: 3,
    media_path: 'C:\\state\\media', registry_path: 'C:\\state\\registry',
    ssh_port: 22240, management_peer: '192.168.199.1', network_prefix_len: 24,
    network: network(),
    template: { path: 'C:\\templates\\worker.zip', source: 'downloaded', version: '1.0.0', sha256: 'a'.repeat(64) },
    instructions: { summary: 'fixture', setup_executable: 'WukongWorkerSetup.exe', receipt_filename: 'connection.json', steps: [] },
  };
}

function importResult(state: 'available' | 'installed_pending'): WorkerMediaImportResult {
  return {
    installation_id: INSTALLATION_ID, environment_id: environmentId, state,
    environment: { environment_id: environmentId, name: 'Windows Worker 01234567', ssh_host: '192.168.199.40' },
    address: state === 'available' ? '192.168.199.40' : null,
    candidate_count: 1,
    guidance: state === 'installed_pending' ? 'fixture guidance' : null,
  };
}

function receiptFile() {
  const receipt = {
    schema: 1, receipt_version: 2, installed: true,
    installation_id: INSTALLATION_ID, environment_id: environmentId, generation: 1,
    manifest_sha256: 'b'.repeat(64), ssh_user: 'wkgm0123456789abcdef', ssh_port: 22240,
    host_key: { algorithm: 'ssh-ed25519', key: 'fixture-key' },
    host_fingerprint: 'SHA256:fixture-fingerprint',
    candidate_addresses: ['192.168.199.40'], production_isolation_verified: false,
  };
  return { text: JSON.stringify(receipt), file: new File([JSON.stringify(receipt)], 'connection.json', { type: 'application/json' }) };
}

function BumpedCard({ onRegistered = vi.fn() }: { onRegistered?: () => void }) {
  const [epoch, setEpoch] = useState(1);
  return <WindowsWorker epoch={epoch} disabled={false} onRegistered={() => { onRegistered(); setEpoch(value => value + 1); }} />;
}

test('an empty controller explains the two-step flow and offers exactly two primary actions', async () => {
  useApi({ worker_media_status: () => status() });
  const { container } = render(<WindowsWorker epoch={1} disabled={false} onRegistered={vi.fn()} />);
  await screen.findByText(/尚未生成安装包/);
  expect(container.querySelectorAll('button.primary')).toHaveLength(2);
  expect((screen.getByRole('button', { name: '生成安装包' }) as HTMLButtonElement).disabled).toBe(false);
  expect((screen.getByRole('button', { name: '导入安装结果' }) as HTMLButtonElement).disabled).toBe(true);
});

test('generation uses the recommended network and accepts a local template fallback', async () => {
  const calls: { epoch: number }[] = [];
  const generated: any[] = [];
  useApi({
    worker_media_status: (args: { epoch: number }) => { calls.push(args); return status(); },
    worker_media_networks: () => [network(), network({ id: 'wifi', name: 'Wi-Fi', ipv4: '10.0.0.5', virtual_adapter: false, recommended: false })],
    pick_worker_output: () => 'C:\\out\\worker.zip',
    pick_worker_template: () => 'C:\\templates\\worker-template.zip',
    worker_media_generate: (args: unknown) => { generated.push(args); return generation(); },
  });
  render(<WindowsWorker epoch={1} disabled={false} onRegistered={vi.fn()} />);
  fireEvent.click(await screen.findByRole('button', { name: '生成安装包' }));
  const dialog = await screen.findByRole('dialog');
  await waitFor(() => expect((within(dialog).getByLabelText('管理网络') as HTMLSelectElement).value).toBe('192.168.199.1/24'));
  fireEvent.change(within(dialog).getByPlaceholderText('Windows Worker'), { target: { value: 'VM worker' } });
  fireEvent.click(within(dialog).getByRole('button', { name: '选择保存位置' }));
  await waitFor(() => expect((within(dialog).getByLabelText('安装包保存位置') as HTMLInputElement).value).toBe('C:\\out\\worker.zip'));
  fireEvent.click(within(dialog).getByRole('button', { name: '已有下载好的模板 ZIP？' }));
  await waitFor(() => expect(within(dialog).getByText('C:\\templates\\worker-template.zip')).toBeTruthy());
  fireEvent.click(within(dialog).getByRole('button', { name: '生成' }));
  await waitFor(() => expect(generated).toEqual([{
    epoch: 1, output: 'C:\\out\\worker.zip', template: 'C:\\templates\\worker-template.zip',
    environmentName: 'VM worker', network: '192.168.199.1/24',
  }]));
  await waitFor(() => expect(calls).toEqual([{ epoch: 1 }, { epoch: 1 }]));
});

test('a release without a media template points at the local template fallback', async () => {
  const generated: any[] = [];
  useApi({
    worker_media_status: () => status(),
    worker_media_networks: () => [network({ recommended: false })],
    pick_worker_output: () => 'C:\\out\\worker.zip',
    pick_worker_template: () => 'C:\\templates\\local.zip',
    worker_media_generate: (args: unknown) => {
      generated.push(args);
      if (generated.length === 1) throw { code: 'error-operation', args: {}, detail: 'this release does not publish a Windows worker media template; choose a local template file' };
      return generation();
    },
  });
  render(<WindowsWorker epoch={1} disabled={false} onRegistered={vi.fn()} />);
  fireEvent.click(await screen.findByRole('button', { name: '生成安装包' }));
  const dialog = await screen.findByRole('dialog');
  fireEvent.click(within(dialog).getByRole('button', { name: '选择保存位置' }));
  await waitFor(() => expect((within(dialog).getByLabelText('安装包保存位置') as HTMLInputElement).value).toBe('C:\\out\\worker.zip'));
  fireEvent.click(within(dialog).getByRole('button', { name: '生成' }));
  await waitFor(() => expect(within(dialog).getByText(/此发行版未发布 Windows Worker 介质模板/)).toBeTruthy());
  expect(screen.getByRole('dialog')).toBeTruthy();
  fireEvent.click(within(dialog).getByRole('button', { name: '已有下载好的模板 ZIP？' }));
  await waitFor(() => expect(within(dialog).getByText('C:\\templates\\local.zip')).toBeTruthy());
  fireEvent.click(within(dialog).getByRole('button', { name: '生成' }));
  await waitFor(() => expect(generated).toHaveLength(2));
  expect(generated[1]).toMatchObject({ template: 'C:\\templates\\local.zip' });
  expect(generated[1]).not.toHaveProperty('network');
});

test('importing a receipt requires the independently typed fingerprint and refreshes with the new epoch', async () => {
  const calls: number[] = [];
  const imported: any[] = [];
  const registered = vi.fn();
  useApi({
    worker_media_status: (args: { epoch: number }) => {
      calls.push(args.epoch);
      return args.epoch === 1 ? status([installation()]) : status([installation({
        state: 'available', registered_environment_id: environmentId, registered_environment_name: 'Windows Worker 01234567',
      })]);
    },
    worker_media_import: (args: unknown) => { imported.push(args); return importResult('available'); },
  });
  render(<BumpedCard onRegistered={registered} />);
  fireEvent.click((await screen.findAllByRole('button', { name: '导入安装结果' }))[0]);
  const dialog = await screen.findByRole('dialog');
  const file = receiptFile();
  fireEvent.change(within(dialog).getByLabelText('安装结果文件'), { target: { files: [file.file] } });
  await waitFor(() => expect(within(dialog).getByText('SHA256:fixture-fingerprint')).toBeTruthy());
  const action = within(dialog).getByRole('button', { name: '导入并校验' }) as HTMLButtonElement;
  expect(action.disabled).toBe(true);
  const confirmation = within(dialog).getByLabelText('虚拟机安装窗口显示的 SSH 主机指纹');
  fireEvent.change(confirmation, { target: { value: 'SHA256:wrong' } });
  expect(within(dialog).getByRole('alert').textContent).toContain('两个主机指纹不一致');
  expect(action.disabled).toBe(true);
  fireEvent.change(confirmation, { target: { value: 'SHA256:fixture-fingerprint' } });
  expect((within(dialog).getByPlaceholderText('Windows Worker') as HTMLInputElement).value).toBe('Windows Worker 01234567');
  expect(action.disabled).toBe(false);
  fireEvent.click(action);
  await waitFor(() => expect(imported).toEqual([{
    epoch: 1, receipt: file.text, confirmedFingerprint: 'SHA256:fixture-fingerprint', name: 'Windows Worker 01234567',
  }]));
  expect(registered).toHaveBeenCalledOnce();
  await waitFor(() => expect(calls).toContain(2));
  await screen.findByText(/Windows Worker 可用，地址 192\.168\.199\.40/);
});

test('a pending installation is retried with the new epoch and becomes available', async () => {
  const calls: number[] = [];
  const retried: any[] = [];
  const registered = vi.fn();
  useApi({
    worker_media_status: (args: { epoch: number }) => {
      calls.push(args.epoch);
      return args.epoch === 1
        ? status([installation({ state: 'installed_pending', candidate_addresses: ['192.168.199.40'], last_address: '192.168.199.40' })])
        : status([installation({ state: 'available', registered_environment_id: environmentId, registered_environment_name: 'Windows Worker 01234567' })]);
    },
    worker_media_retry: (args: unknown) => { retried.push(args); return importResult('available'); },
  });
  render(<BumpedCard onRegistered={registered} />);
  fireEvent.click(await screen.findByRole('button', { name: '重试连接' }));
  await waitFor(() => expect(retried).toEqual([{ epoch: 1, installationId: INSTALLATION_ID }]));
  expect(registered).toHaveBeenCalledOnce();
  await waitFor(() => expect(calls).toContain(2));
  await screen.findByText(/Windows Worker 可用，地址 192\.168\.199\.40/);
  expect(screen.getByText('可用')).toBeTruthy();
});

test('a foreign or host-key mismatch error keeps the import dialog open with an actionable hint', async () => {
  useApi({
    worker_media_status: () => status([installation()]),
    worker_media_import: () => { throw { code: 'error-operation', args: {}, detail: 'a registered environment pins a different worker host key; the connection was not replaced' }; },
  });
  render(<WindowsWorker epoch={1} disabled={false} onRegistered={vi.fn()} />);
  fireEvent.click((await screen.findAllByRole('button', { name: '导入安装结果' }))[0]);
  const dialog = await screen.findByRole('dialog');
  const file = receiptFile();
  fireEvent.change(within(dialog).getByLabelText('安装结果文件'), { target: { files: [file.file] } });
  await waitFor(() => expect(within(dialog).getByText('SHA256:fixture-fingerprint')).toBeTruthy());
  fireEvent.change(within(dialog).getByLabelText('虚拟机安装窗口显示的 SSH 主机指纹'), { target: { value: 'SHA256:fixture-fingerprint' } });
  fireEvent.click(within(dialog).getByRole('button', { name: '导入并校验' }));
  await waitFor(() => expect(within(dialog).getByText(/主机密钥与已登记 Worker 不一致/)).toBeTruthy());
  expect(within(dialog).getByRole('alert').textContent).toContain('different worker host key');
});

test('corrupt records, missing records and incomplete verification are distinct states', async () => {
  useApi({
    worker_media_status: () => status([
      installation({
        diagnostics: {
          generation_record: 'present', recovery_record: 'corrupt', receipt: 'unknown',
          registration: 'missing', verification: 'unknown',
          detail: 'read the worker media installation state: invalid JSON',
        },
        warning: 'read the worker media installation state: invalid JSON',
      }),
      installation({
        installation_id: 'f'.repeat(32), environment_id: 'windows-ffffffff',
        state: 'installed_pending', last_address: '192.168.199.40',
        diagnostics: {
          generation_record: 'present', recovery_record: 'present', receipt: 'present',
          registration: 'missing', verification: 'incomplete',
        },
      }),
    ]),
  });
  render(<WindowsWorker epoch={1} disabled={false} onRegistered={vi.fn()} />);
  await screen.findByText('损坏');
  expect(screen.getAllByText('已记录')).toHaveLength(4);
  expect(screen.getAllByText('缺失').length).toBeGreaterThan(0);
  expect(screen.getByText('损坏')).toBeTruthy();
  expect(screen.getByText('无法判断')).toBeTruthy();
  expect(screen.getByText('未完成')).toBeTruthy();
  expect(screen.queryByText('已校验')).toBeNull();
  expect(screen.getAllByLabelText('部署记录')).toHaveLength(2);
});

test('corrupt generation records are reported without hiding readable installations', async () => {
  useApi({
    worker_media_status: () => status(
      [installation({
        state: 'available', registered_environment_id: environmentId,
        registered_environment_name: 'Windows Worker 01234567',
        diagnostics: {
          generation_record: 'present', recovery_record: 'present', receipt: 'present',
          registration: 'present', verification: 'verified',
        },
      })],
      {
        inventory: 'present',
        unreadable_generation_records: [{
          path: 'C:\\state\\worker-media\\registry\\ffffffffffffffffffffffffffffffff.json',
          installation_id: 'f'.repeat(32),
          error: 'parse offline media generation record: expected value',
        }],
      },
    ),
  });
  const { container } = render(<WindowsWorker epoch={1} disabled={false} onRegistered={vi.fn()} />);
  await screen.findByText(/1 个损坏的安装记录已保留/);
  expect(screen.getByText(/损坏的记录已保留在原位/)).toBeTruthy();
  expect(screen.getByText('已校验')).toBeTruthy();
  expect(screen.getByText('Windows Worker 01234567')).toBeTruthy();
  const details = container.querySelector('details');
  fireEvent.click(details!.querySelector('summary')!);
  expect(screen.getByText(/parse offline media generation record/)).toBeTruthy();
});

test('an unreadable installation inventory is reported as a corrupt source', async () => {
  useApi({
    worker_media_status: () => status([], {
      inventory: 'corrupt',
      unreadable_generation_records: [],
      detail: 'offline worker media registry must be a directory',
    }),
  });
  const { container } = render(<WindowsWorker epoch={1} disabled={false} onRegistered={vi.fn()} />);
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain('安装记录目录无法读取');
  expect(alert.textContent).toContain('offline worker media registry must be a directory');
  expect(screen.getByText(/尚未生成安装包/)).toBeTruthy();
  expect(container.querySelector('.worker-installations')).toBeNull();
});
