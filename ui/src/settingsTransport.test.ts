import { describe, expect, it } from 'vitest';
import { editedForm, transportEdit } from './settingsTransport';

describe('transport configuration editing', () => {
  it('replaces the whole backend and drops incompatible pending fields', () => {
    const form = { environments: { lab: { runtime: { backend: 'docker', docker_container: 'worker', docker_executable: 'docker', request_timeout_secs: 60 } } } };
    const edits = transportEdit(form, { 'environments.lab.runtime.docker_container': 'draft' }, 'environments.lab.runtime.backend', 'ssh');
    expect(edits).toEqual({ 'environments.lab.runtime': { backend: 'ssh', ssh_host: '', request_timeout_secs: 60 } });
    const next = { ...edits, 'environments.lab.runtime.ssh_host': 'lab-host' };
    expect(editedForm(form, next)).toMatchObject({ environments: { lab: { runtime: { backend: 'ssh', ssh_host: 'lab-host' } } } });
    expect(edits['environments.lab.runtime']).toEqual({ backend: 'ssh', ssh_host: '', request_timeout_secs: 60 });
    expect(form.environments.lab.runtime.docker_container).toBe('worker');
  });
});
