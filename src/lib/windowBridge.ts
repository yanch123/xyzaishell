import { invoke } from '@tauri-apps/api/core';
export type WindowLaunchTarget = { profile_id: string | null; new_connection: boolean };
/** Only a persisted profile identifier crosses process boundaries. Secrets stay local. */
export const windowBridge = {
  launch: (profileId: string | null = null) => invoke<void>('launch_new_window', { profileId }),
  target: () => invoke<WindowLaunchTarget>('get_window_launch_target'),
};
