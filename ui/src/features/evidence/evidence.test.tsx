import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, vi, it, expect } from 'vitest';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { api } from '../../shared/api/bridge';
import { EvidenceReader, Findings } from './Findings';
import type { EvidenceView, Finding, FindingDetails } from '../../shared/api/types';
import { finding } from '../../shared/testing/fixtures';

vi.mock('../../shared/api/bridge', () => ({ api: vi.fn(), errorText: String, errorValue: String }));

const evidence = { id: 'a', agent_id: 'agent', tool_call_id: 'call', source: 'a.txt', media_type: 'text/plain', sha256: 'hash', size: 8 };
function preview(id: string, text: string, offset = 0, end = 4): EvidenceView {
  return { ...evidence, id, offset, end, text, binary: false };
}
function details(value: Finding, impact: string): FindingDetails {
  return { finding: value, verification: { passed: true, attempts: 1, successes: 1, impact, environment: {}, conditions: {}, result: {} }, evidence: [], calls: [], missing_evidence: [] };
}
function queryClient() {
  return new QueryClient({ defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false, refetchOnReconnect: false } } });
}
function withQuery(client: QueryClient, children: ReactNode) {
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}
beforeEach(() => { vi.mocked(api).mockReset(); });

it('never shows an old evidence item or run in the current reader', async () => {
  let resolveOld!: (value: EvidenceView) => void;
  vi.mocked(api<'evidence_preview'>).mockImplementation((_command, args) => {
    if (args?.id === 'a') return new Promise(resolve => { resolveOld = resolve; });
    return Promise.resolve(preview('b', args?.run === 'other' ? 'other run evidence' : 'new evidence'));
  });
  const client = queryClient();
  const view = render(withQuery(client, <EvidenceReader key="a" evidence={evidence} epoch={1} run="run" />));
  view.rerender(withQuery(client, <EvidenceReader key="b" evidence={{ ...evidence, id: 'b' }} epoch={1} run="run" />));
  await screen.findByText('new evidence');
  view.rerender(withQuery(client, <EvidenceReader key="b" evidence={{ ...evidence, id: 'b' }} epoch={2} run="other" />));
  await screen.findByText('other run evidence');
  await act(async () => resolveOld(preview('a', 'old evidence')));
  expect(screen.queryByText('old evidence')).toBeNull();
  expect(screen.getByText('other run evidence')).toBeTruthy();
});

it('clears the previous evidence page immediately while the next page loads', async () => {
  let resolveNext!: (value: EvidenceView) => void;
  vi.mocked(api<'evidence_preview'>).mockImplementation((_command, args) => args?.offset === 0
    ? Promise.resolve(preview('a', 'first page', 0, 4))
    : new Promise(resolve => { resolveNext = resolve; }));
  render(withQuery(queryClient(), <EvidenceReader evidence={evidence} epoch={1} run="run" />));
  await screen.findByText('first page');
  fireEvent.click(screen.getByLabelText('下一页证据'));
  expect(screen.queryByText('first page')).toBeNull();
  await act(async () => resolveNext(preview('a', 'second page', 4, 8)));
  await screen.findByText('second page');
});

it('does not retry evidence or finding read failures', async () => {
  vi.mocked(api<'evidence_preview'>).mockRejectedValue(new Error('evidence failed'));
  const view = render(withQuery(queryClient(), <EvidenceReader evidence={evidence} epoch={1} run="run" />));
  await screen.findByRole('alert');
  expect(api).toHaveBeenCalledTimes(1);
  view.unmount();

  vi.mocked(api).mockReset();
  vi.mocked(api<'finding_details'>).mockRejectedValue(new Error('finding failed'));
  render(withQuery(queryClient(), <Findings findings={[finding]} epoch={1} run="run" onAgent={vi.fn()} />));
  await screen.findByRole('alert');
  expect(api).toHaveBeenCalledTimes(1);
});

it('isolates finding details by item, update and run and ignores late responses', async () => {
  let resolveOld!: (value: FindingDetails) => void;
  let currentVersion = '2026-01-01';
  vi.mocked(api<'finding_details'>).mockImplementation((_command, args) => {
    if (args?.id === 'old') return new Promise(resolve => { resolveOld = resolve; });
    const current = { ...finding, id: args!.id, updated_at: currentVersion };
    return Promise.resolve(details(current, `${args?.run}:${args?.epoch}:${currentVersion}`));
  });
  const old = { ...finding, id: 'old', updated_at: '2026-01-01' };
  const current = { ...finding, id: 'current', updated_at: '2026-01-01' };
  const client = queryClient();
  const view = render(withQuery(client, <Findings findings={[old]} epoch={1} run="run" onAgent={vi.fn()} />));
  await waitFor(() => expect(api).toHaveBeenCalledTimes(1));
  view.rerender(withQuery(client, <Findings findings={[current]} epoch={1} run="run" onAgent={vi.fn()} />));
  await screen.findByText('run:1:2026-01-01');
  currentVersion = '2026-01-02';
  const updated = { ...current, updated_at: currentVersion };
  view.rerender(withQuery(client, <Findings findings={[updated]} epoch={1} run="run" onAgent={vi.fn()} />));
  await screen.findByText('run:1:2026-01-02');
  currentVersion = '2026-01-03';
  const otherRun = { ...current, updated_at: currentVersion };
  view.rerender(withQuery(client, <Findings findings={[otherRun]} epoch={2} run="other" onAgent={vi.fn()} />));
  await screen.findByText('other:2:2026-01-03');
  await act(async () => resolveOld(details(old, 'old late verification')));
  expect(screen.queryByText('old late verification')).toBeNull();
  expect(screen.getByText('other:2:2026-01-03')).toBeTruthy();
});
