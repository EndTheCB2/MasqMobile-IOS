import { normalizeBrowserUrl } from './config';

export const ENS_GATEWAY_SUFFIX = '.limo';
export const ENS_GATEWAY_LABEL = 'eth.limo';

export interface BrowserTarget {
  displayUrl: string;
  isEns: boolean;
  transportUrl: string;
}

/**
 * Converts a user-entered or navigated HTTPS URL into the URL WebView may
 * actually load. ENS names keep their logical `.eth` address for display, but
 * use eth.limo as an explicit HTTPS transport boundary for the MVP.
 */
export function resolveBrowserUrlTarget(input: string): BrowserTarget {
  const normalized = normalizeBrowserUrl(input);
  const parsed = new URL(normalized);
  const hostname = parsed.hostname.toLowerCase().replace(/\.$/, '');

  if (isEnsGatewayHostname(hostname)) {
    const ensHostname = hostname.slice(0, -ENS_GATEWAY_SUFFIX.length);
    assertSafeEnsHostname(ensHostname);
    assertNoCredentialsOrPort(parsed);
    return {
      displayUrl: replaceUrlHostname(parsed, ensHostname),
      isEns: true,
      transportUrl: parsed.toString(),
    };
  }

  if (!isEnsHostname(hostname)) {
    return {
      displayUrl: parsed.toString(),
      isEns: false,
      transportUrl: parsed.toString(),
    };
  }

  assertSafeEnsHostname(hostname);
  assertNoCredentialsOrPort(parsed);
  return {
    displayUrl: replaceUrlHostname(parsed, hostname),
    isEns: true,
    transportUrl: replaceUrlHostname(
      parsed,
      `${hostname}${ENS_GATEWAY_SUFFIX}`,
    ),
  };
}

export function displayBrowserUrl(url: string): string {
  try {
    return resolveBrowserUrlTarget(url).displayUrl;
  } catch {
    return url;
  }
}

export function browserTargetHostname(target: BrowserTarget): string {
  return new URL(target.transportUrl).hostname.toLowerCase();
}

export function isEnsHostname(hostname: string): boolean {
  const normalized = hostname.toLowerCase().replace(/\.$/, '');
  return normalized === 'eth' || normalized.endsWith('.eth');
}

export function isEnsGatewayHostname(hostname: string): boolean {
  const normalized = hostname.toLowerCase().replace(/\.$/, '');
  return normalized.endsWith(`.eth${ENS_GATEWAY_SUFFIX}`);
}

function assertNoCredentialsOrPort(parsed: URL): void {
  if (parsed.username || parsed.password || parsed.port) {
    throw new Error(
      'ENS website addresses cannot contain credentials or ports.',
    );
  }
}

function replaceUrlHostname(parsed: URL, hostname: string): string {
  return `https://${hostname}${parsed.pathname}${parsed.search}${parsed.hash}`;
}

function assertSafeEnsHostname(hostname: string): void {
  const normalized = hostname.toLowerCase().replace(/\.$/, '');
  const labels = normalized.split('.');
  if (
    normalized.length > 253 ||
    labels.length < 2 ||
    labels[labels.length - 1] !== 'eth'
  ) {
    throw new Error('Enter a valid ENS .eth website.');
  }

  for (const label of labels) {
    if (
      label.length < 1 ||
      label.length > 63 ||
      !/^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(label)
    ) {
      throw new Error(
        'This ENS name is not safely normalized. Use its normalized or punycode form.',
      );
    }
  }
}
