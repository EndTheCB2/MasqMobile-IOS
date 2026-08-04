export type SystemTunnelMode = 'off' | 'wholeDevice' | 'selectedApps';
export type SystemTunnelPhase =
  | 'off'
  | 'starting'
  | 'active'
  | 'stopping'
  | 'blocked';
export type SystemTunnelRoutingPhase =
  | 'off'
  | 'requestingPermission'
  | 'startingBlocking'
  | 'reconnecting'
  | 'active'
  | 'blocked'
  | 'stopping'
  | 'revoked';
export type SystemTunnelTrafficDisposition =
  | 'masq'
  | 'blocked'
  | 'directRisk'
  | 'off';

export interface SystemTunnelStatus {
  active: boolean;
  alwaysOn?: boolean;
  appliedMode?: SystemTunnelMode;
  appliedRevision?: number | null;
  appliedSelectedApps?: string[];
  coreRouteReady?: boolean;
  desiredMode?: SystemTunnelMode;
  desiredRevision?: number | null;
  desiredSelectedApps?: string[];
  failClosedDesired?: boolean;
  lastError: string | null;
  lockdown?: boolean;
  mode: SystemTunnelMode;
  phase: SystemTunnelPhase;
  routingPhase?: SystemTunnelRoutingPhase;
  schemaVersion?: number;
  selectedApps: string[];
  supported: boolean;
  trafficDisposition?: SystemTunnelTrafficDisposition;
  trafficObserved?: boolean;
  translatorReady?: boolean;
  tunPresent?: boolean;
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
  const versioned = wireStatus.schemaVersion === 2;
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
    (hasSchemaVersion && !versioned) ||
    (versioned &&
      (!hasTrafficDisposition ||
        !isVersionedSystemTunnelStatus(status, wireStatus))) ||
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

function isVersionedSystemTunnelStatus(
  status: Partial<SystemTunnelStatus>,
  wireStatus: Record<string, unknown>,
): boolean {
  const desiredMode = status.desiredMode;
  const appliedMode = status.appliedMode;
  const desiredApps = status.desiredSelectedApps;
  const appliedApps = status.appliedSelectedApps;
  return (
    [
      'active',
      'alwaysOn',
      'coreRouteReady',
      'failClosedDesired',
      'lockdown',
      'supported',
      'trafficObserved',
      'translatorReady',
      'tunPresent',
    ].every(field => typeof wireStatus[field] === 'boolean') &&
    [
      'off',
      'requestingPermission',
      'startingBlocking',
      'reconnecting',
      'active',
      'blocked',
      'stopping',
      'revoked',
    ].includes(String(status.routingPhase)) &&
    ['off', 'wholeDevice', 'selectedApps'].includes(desiredMode || '') &&
    ['off', 'wholeDevice', 'selectedApps'].includes(appliedMode || '') &&
    validRevision(status.desiredRevision) &&
    validRevision(status.appliedRevision) &&
    validScope(desiredMode, status.desiredRevision, desiredApps) &&
    validScope(appliedMode, status.appliedRevision, appliedApps) &&
    status.mode === desiredMode &&
    Array.isArray(status.selectedApps) &&
    sameApps(status.selectedApps, desiredApps || [])
  );
}

function validRevision(value: unknown): value is number | null {
  return (
    value === null ||
    (typeof value === 'number' && Number.isSafeInteger(value) && value > 0)
  );
}

function validScope(
  mode: SystemTunnelMode | undefined,
  revision: number | null | undefined,
  apps: string[] | undefined,
): boolean {
  if (
    !mode ||
    !Array.isArray(apps) ||
    !apps.every(app => typeof app === 'string' && app.length > 0)
  ) {
    return false;
  }
  if (mode === 'off') {
    return apps.length === 0;
  }
  if (revision === null || revision === undefined) {
    return false;
  }
  return mode === 'selectedApps' ? apps.length > 0 : apps.length === 0;
}

export function systemTunnelTrafficDisposition(
  status: SystemTunnelStatus,
): SystemTunnelTrafficDisposition {
  if (status.trafficDisposition) {
    if (status.trafficDisposition !== 'masq') {
      return status.trafficDisposition;
    }
    if (isVerifiedSystemTunnelRoute(status)) {
      return 'masq';
    }
    return status.tunPresent ? 'blocked' : 'directRisk';
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

export function isVerifiedSystemTunnelRoute(
  status: SystemTunnelStatus,
): boolean {
  if (
    !status.active ||
    status.phase !== 'active' ||
    (status.routingPhase !== undefined && status.routingPhase !== 'active')
  ) {
    return false;
  }
  if (status.schemaVersion !== 2) {
    return true;
  }
  const desired = desiredSystemTunnelScope(status);
  const applied = appliedSystemTunnelScope(status);
  return (
    status.trafficDisposition === 'masq' &&
    status.tunPresent === true &&
    status.translatorReady === true &&
    status.coreRouteReady === true &&
    desired.revision !== null &&
    desired.revision !== 'legacy' &&
    desired.revision === applied.revision &&
    desired.mode !== 'off' &&
    desired.mode === applied.mode &&
    sameApps(desired.selectedApps, applied.selectedApps)
  );
}

export function desiredSystemTunnelScope(status: SystemTunnelStatus): {
  mode: SystemTunnelMode;
  revision: number | null | 'legacy';
  selectedApps: string[];
} {
  const { desiredMode, desiredRevision, desiredSelectedApps } = status;
  const hasCompositeScope =
    desiredMode !== undefined &&
    desiredRevision !== undefined &&
    desiredSelectedApps !== undefined;
  return hasCompositeScope
    ? {
        mode: desiredMode,
        revision: desiredRevision,
        selectedApps: desiredSelectedApps,
      }
    : {
        mode: status.mode,
        revision: 'legacy',
        selectedApps: status.selectedApps,
      };
}

export function desiredSystemTunnelScopeKey(
  status: SystemTunnelStatus,
): string {
  return systemTunnelScopeKey(desiredSystemTunnelScope(status));
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
  return systemTunnelScopeKey(appliedSystemTunnelScope(status));
}

function systemTunnelScopeKey(scope: {
  mode: SystemTunnelMode;
  revision: number | null | 'legacy';
  selectedApps: string[];
}): string {
  return [
    scope.revision === null || scope.revision === 'legacy'
      ? 'none'
      : String(scope.revision),
    scope.mode,
    ...[...scope.selectedApps].sort(),
  ].join('\u0000');
}

function sameApps(first: string[], second: string[]): boolean {
  const sortedFirst = [...first].sort();
  const sortedSecond = [...second].sort();
  return (
    first.length === second.length &&
    sortedFirst.every((app, index) => app === sortedSecond[index])
  );
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
