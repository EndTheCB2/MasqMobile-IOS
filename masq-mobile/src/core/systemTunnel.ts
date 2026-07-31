export type SystemTunnelMode = 'off' | 'wholeDevice' | 'selectedApps';
export type SystemTunnelPhase =
  | 'off'
  | 'starting'
  | 'active'
  | 'stopping'
  | 'blocked';
export type SystemTunnelTrafficDisposition =
  | 'masq'
  | 'blocked'
  | 'directRisk'
  | 'off';

export interface SystemTunnelStatus {
  active: boolean;
  appliedMode?: SystemTunnelMode;
  appliedRevision?: number | null;
  appliedSelectedApps?: string[];
  lastError: string | null;
  mode: SystemTunnelMode;
  phase: SystemTunnelPhase;
  selectedApps: string[];
  supported: boolean;
  trafficDisposition?: SystemTunnelTrafficDisposition;
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
  trafficDisposition: 'off',
};

export function decodeSystemTunnelStatus(
  serialized: string,
): SystemTunnelStatus {
  const parsed: unknown = JSON.parse(serialized);
  if (!parsed || typeof parsed !== 'object') {
    throw new Error('The native system tunnel returned invalid data.');
  }
  const status = parsed as Partial<SystemTunnelStatus>;
  const wireStatus = parsed as Record<string, unknown>;
  const hasSchemaVersion = Object.prototype.hasOwnProperty.call(
    wireStatus,
    'schemaVersion',
  );
  const hasTrafficDisposition = Object.prototype.hasOwnProperty.call(
    wireStatus,
    'trafficDisposition',
  );
  const hasAppliedScope =
    Object.prototype.hasOwnProperty.call(status, 'appliedRevision') ||
    Object.prototype.hasOwnProperty.call(status, 'appliedMode') ||
    Object.prototype.hasOwnProperty.call(status, 'appliedSelectedApps');
  if (
    typeof status.supported !== 'boolean' ||
    typeof status.active !== 'boolean' ||
    (hasSchemaVersion && wireStatus.schemaVersion !== 2) ||
    (wireStatus.schemaVersion === 2 && !hasTrafficDisposition) ||
    (hasTrafficDisposition &&
      !['masq', 'blocked', 'directRisk', 'off'].includes(
        String(status.trafficDisposition),
      )) ||
    !['off', 'wholeDevice', 'selectedApps'].includes(status.mode || '') ||
    !['off', 'starting', 'active', 'stopping', 'blocked'].includes(
      status.phase || '',
    ) ||
    !Array.isArray(status.selectedApps) ||
    !status.selectedApps.every(
      app => typeof app === 'string' && app.length > 0,
    ) ||
    (typeof status.lastError !== 'string' && status.lastError !== null) ||
    (hasAppliedScope &&
      (!Object.prototype.hasOwnProperty.call(status, 'appliedRevision') ||
        (status.appliedRevision !== null &&
          (typeof status.appliedRevision !== 'number' ||
            !Number.isSafeInteger(status.appliedRevision) ||
            status.appliedRevision <= 0)) ||
        !['off', 'wholeDevice', 'selectedApps'].includes(
          status.appliedMode || '',
        ) ||
        !Array.isArray(status.appliedSelectedApps) ||
        !status.appliedSelectedApps.every(
          app => typeof app === 'string' && app.length > 0,
        ) ||
        (status.appliedMode !== 'off' && status.appliedRevision === null) ||
        (status.appliedMode === 'selectedApps'
          ? status.appliedSelectedApps.length === 0
          : status.appliedSelectedApps.length !== 0)))
  ) {
    throw new Error('The native system tunnel returned invalid data.');
  }
  return status as SystemTunnelStatus;
}

export function systemTunnelTrafficDisposition(
  status: SystemTunnelStatus,
): SystemTunnelTrafficDisposition {
  if (status.trafficDisposition) {
    return status.trafficDisposition;
  }
  if (status.active && status.phase === 'active') {
    return 'masq';
  }
  if (status.phase === 'blocked' || status.phase === 'stopping') {
    return 'blocked';
  }
  if (status.mode !== 'off') {
    return 'directRisk';
  }
  return 'off';
}

export function appliedSystemTunnelScope(status: SystemTunnelStatus): {
  mode: SystemTunnelMode;
  revision: number | null | 'legacy';
  selectedApps: string[];
} {
  const { appliedMode, appliedRevision, appliedSelectedApps } = status;
  const hasCompositeScope =
    appliedMode !== undefined &&
    appliedRevision !== undefined &&
    appliedSelectedApps !== undefined;
  return hasCompositeScope
    ? {
        mode: appliedMode,
        revision: appliedRevision,
        selectedApps: appliedSelectedApps,
      }
    : {
        mode: status.mode,
        revision: 'legacy',
        selectedApps: status.selectedApps,
      };
}

export function appliedSystemTunnelScopeKey(
  status: SystemTunnelStatus,
): string {
  const scope = appliedSystemTunnelScope(status);
  return [
    scope.revision === null || scope.revision === 'legacy'
      ? 'none'
      : String(scope.revision),
    scope.mode,
    ...[...scope.selectedApps].sort(),
  ].join('\u0000');
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
