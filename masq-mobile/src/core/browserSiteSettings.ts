import type { BrowserRoutingMode } from './masqCore';

export type BrowserSiteMode = Exclude<BrowserRoutingMode, 'blocked'>;

export interface BrowserSiteSettings {
  hostname: string;
  mode: BrowserSiteMode;
  persistentSessionsSupported: boolean;
  protectionDisabled: boolean;
  rememberSignIn: boolean;
}

const SETTINGS_KEYS = [
  'hostname',
  'mode',
  'persistentSessionsSupported',
  'protectionDisabled',
  'rememberSignIn',
] as const;

export function browserSiteHostname(url: string): string {
  const parsed = new URL(url);
  if (
    parsed.protocol !== 'https:' ||
    parsed.username ||
    parsed.password ||
    parsed.port
  ) {
    throw new Error('Site settings require a public HTTPS website.');
  }
  const hostname = parsed.hostname.toLowerCase().replace(/\.$/, '');
  if (!isSafeBrowserSiteHostname(hostname)) {
    throw new Error('Site settings require a valid public hostname.');
  }
  return hostname;
}

export function decodeBrowserSiteSettings(
  serialized: string,
): BrowserSiteSettings {
  let parsed: unknown;
  try {
    parsed = JSON.parse(serialized);
  } catch {
    throw new Error('The native browser site settings are not valid JSON.');
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('The native browser site settings are invalid.');
  }
  const record = parsed as Record<string, unknown>;
  const keys = Object.keys(record).sort();
  const expected = [...SETTINGS_KEYS].sort();
  if (
    keys.length !== expected.length ||
    keys.some((key, index) => key !== expected[index])
  ) {
    throw new Error(
      'The native browser site settings have unsupported fields.',
    );
  }
  if (
    typeof record.hostname !== 'string' ||
    !isSafeBrowserSiteHostname(record.hostname) ||
    (record.mode !== 'masq' && record.mode !== 'direct') ||
    typeof record.persistentSessionsSupported !== 'boolean' ||
    typeof record.protectionDisabled !== 'boolean' ||
    typeof record.rememberSignIn !== 'boolean'
  ) {
    throw new Error('The native browser site settings have invalid values.');
  }
  if (record.rememberSignIn && !record.persistentSessionsSupported) {
    throw new Error(
      'Browser site settings enabled remembered sign-in without isolated profile support.',
    );
  }
  return {
    hostname: record.hostname,
    mode: record.mode,
    persistentSessionsSupported: record.persistentSessionsSupported,
    protectionDisabled: record.protectionDisabled,
    rememberSignIn: record.rememberSignIn,
  };
}

export function isSafeBrowserSiteHostname(hostname: string): boolean {
  if (
    hostname.length < 1 ||
    hostname.length > 253 ||
    hostname !== hostname.toLowerCase() ||
    hostname.endsWith('.') ||
    hostname === 'localhost' ||
    hostname.endsWith('.local')
  ) {
    return false;
  }
  const labels = hostname.split('.');
  return (
    labels.length >= 2 &&
    labels.every(
      label =>
        label.length >= 1 &&
        label.length <= 63 &&
        /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(label),
    )
  );
}
