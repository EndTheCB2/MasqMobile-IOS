export type SystemTunnelMode = 'off' | 'wholeDevice' | 'selectedApps';
export type SystemTunnelPhase =
  | 'off'
  | 'starting'
  | 'active'
  | 'stopping'
  | 'blocked';

export interface SystemTunnelStatus {
  active: boolean;
  lastError: string | null;
  mode: SystemTunnelMode;
  phase: SystemTunnelPhase;
  selectedApps: string[];
  supported: boolean;
}

export interface RoutableApp {
  id: string;
  label: string;
}

export const UNSUPPORTED_SYSTEM_TUNNEL: SystemTunnelStatus = {
  active: false,
  lastError: null,
  mode: 'off',
  phase: 'off',
  selectedApps: [],
  supported: false,
};

export function decodeSystemTunnelStatus(
  serialized: string,
): SystemTunnelStatus {
  const parsed: unknown = JSON.parse(serialized);
  if (!parsed || typeof parsed !== 'object') {
    throw new Error('The native system tunnel returned invalid data.');
  }
  const status = parsed as Partial<SystemTunnelStatus>;
  if (
    typeof status.supported !== 'boolean' ||
    typeof status.active !== 'boolean' ||
    !['off', 'wholeDevice', 'selectedApps'].includes(status.mode || '') ||
    !['off', 'starting', 'active', 'stopping', 'blocked'].includes(
      status.phase || '',
    ) ||
    !Array.isArray(status.selectedApps) ||
    !status.selectedApps.every(
      app => typeof app === 'string' && app.length > 0,
    ) ||
    (typeof status.lastError !== 'string' && status.lastError !== null)
  ) {
    throw new Error('The native system tunnel returned invalid data.');
  }
  return status as SystemTunnelStatus;
}

export function decodeRoutableApps(serialized: string): RoutableApp[] {
  const parsed: unknown = JSON.parse(serialized);
  if (
    !Array.isArray(parsed) ||
    !parsed.every(
      app =>
        app &&
        typeof app === 'object' &&
        typeof app.id === 'string' &&
        app.id.length > 0 &&
        typeof app.label === 'string' &&
        app.label.length > 0,
    )
  ) {
    throw new Error('The native app list returned invalid data.');
  }
  return parsed as RoutableApp[];
}
