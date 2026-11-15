import type { Json } from './types';

type Form = Record<string, Json>;
export function editedForm(form: Form, edits: Form): Form {
  const draft = structuredClone(form);
  for (const [path, value] of Object.entries(edits)) {
    const keys = path.split('.');
    let parent = draft;
    for (const key of keys.slice(0, -1)) {
      const child = parent[key];
      if (!child || typeof child !== 'object' || Array.isArray(child)) parent[key] = {};
      parent = parent[key] as Form;
    }
    parent[keys.at(-1)!] = structuredClone(value);
  }
  return draft;
}

/** Replace the transport variant as a unit, excluding fields from the former backend. */
export function transportEdit(form: Form, edits: Form, path: string, backend: string): Form {
  const prefix = path.slice(0, -'.backend'.length);
  let current: Json = editedForm(form, edits);
  for (const key of prefix.split('.')) current = (current as Form)[key];
  const prior = current as Form;
  const transport: Form = { backend, request_timeout_secs: prior.request_timeout_secs ?? 300 };
  const fields = backend === 'docker' ? ['docker_container', 'docker_executable', 'remote_bridge_path', 'remote_state_dir']
    : backend === 'ssh' ? ['ssh_host', 'ssh_executable', 'remote_bridge_path', 'remote_state_dir']
    : backend === 'managed_ssh' ? ['ssh_host', 'ssh_executable', 'managed_ssh']
    : backend === 'trusted_local' ? ['state_dir', 'python', 'bridge_path'] : [];
  for (const field of fields) if (prior[field] != null) transport[field] = prior[field];
  if (backend === 'docker') transport.docker_container ??= '';
  if (backend === 'ssh') transport.ssh_host ??= '';
  return { ...Object.fromEntries(Object.entries(edits).filter(([key]) => key !== prefix && !key.startsWith(`${prefix}.`))), [prefix]: transport };
}
