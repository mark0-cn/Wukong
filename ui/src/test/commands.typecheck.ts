import { api } from '../bridge';
import type { UpdateStatus } from '../commands';

// Compile-only contract checks; never invoke native commands from a type test.
export function commandTypeChecks() {
  const result: Promise<UpdateStatus> = api('update_action', { epoch: 1, action: 'check' });
  void result;
  // @ts-expect-error command names are closed
  void api('unknown_command');
  // @ts-expect-error admission epoch is required
  void api('start_run', { target: 'fixture', task: 'inspect' });
  // @ts-expect-error return type is fixed by the command
  const wrong: Promise<string> = api('update_action', { epoch: 1, action: 'status' });
  void wrong;
  // @ts-expect-error workbench operations have different required arguments
  void api('workbench_action', { epoch: 1, request: { action: 'import', id: 'case' } });
}
