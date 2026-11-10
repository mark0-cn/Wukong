import { render, screen, act } from '@testing-library/react';
import { vi, it, expect } from 'vitest';
import { api } from '../bridge';
import { EvidenceReader } from '../components/Findings';
import type { EvidenceView } from '../types';
vi.mock('../bridge', () => ({ api: vi.fn(), errorText: String }));
it('never shows an old evidence page in the newly selected evidence', async () => {
  const evidence = { id: 'a', agent_id: 'agent', tool_call_id: 'call', source: 'a.txt', media_type: 'text/plain', sha256: 'hash', size: 4 };
  let resolveOld!: (value: EvidenceView) => void;
  vi.mocked(api<'evidence_preview'>).mockImplementation((_command, args) => args?.id === 'a' ? new Promise(resolve => { resolveOld = resolve as typeof resolveOld; }) : Promise.resolve({ ...evidence, id: 'b', offset: 0, end: 4, text: 'new evidence', binary: false }));
  const view = render(<EvidenceReader key="a" evidence={evidence} epoch={1} run="run" />);
  view.rerender(<EvidenceReader key="b" evidence={{ ...evidence, id: 'b' }} epoch={1} run="run" />);
  await screen.findByText('new evidence');
  await act(async () => resolveOld({ ...evidence, offset: 0, end: 4, text: 'old evidence', binary: false }));
  expect(screen.queryByText('old evidence')).toBeNull();
  expect(screen.getByText('new evidence')).toBeTruthy();
});
