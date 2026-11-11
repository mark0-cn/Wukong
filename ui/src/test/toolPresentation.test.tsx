import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { ToolStep } from '../components/Conversation';
import { api } from '../bridge';
import { tool } from './fixtures';

vi.mock('../bridge', () => ({ api: vi.fn(), errorText: String, errorValue: String, openLink: vi.fn() }));

describe('evidence-backed tool presentation', () => {
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
