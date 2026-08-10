import type { Chain, MasqConfig, SetupDraft } from './types';

const PRIVATE_KEY_PATTERN = /^(?:0x)?[a-fA-F0-9]{64}$/;
const HOST_PATTERN = '(?:\\[[0-9a-fA-F:]+\\]|[a-zA-Z0-9.-]+)';
const DESCRIPTOR_PATTERN = new RegExp(
  `^masq:\\/\\/(base-mainnet|base-sepolia):[A-Za-z0-9_-]+@${HOST_PATTERN}:(\\d{1,5})$`,
);

export interface ValidationErrors {
  rpcUrl?: string;
  neighbors?: string;
  minHops?: string;
  exitCountry?: string;
  walletSecret?: string;
}

interface ValidationOptions {
  walletRequired?: boolean;
}

export function parseNeighborList(value: string): string[] {
  return value
    .split(/[\n,]/)
    .map(item => item.trim())
    .filter(Boolean);
}

export function validateConfig(
  draft: SetupDraft,
  options: ValidationOptions = {},
): ValidationErrors {
  const errors: ValidationErrors = {};

  try {
    const rpc = new URL(draft.rpcUrl.trim());
    if (rpc.protocol !== 'https:') {
      errors.rpcUrl = 'Use an HTTPS RPC endpoint.';
    }
    if (rpc.username || rpc.password) {
      errors.rpcUrl = 'Do not include credentials in the RPC URL.';
    }
  } catch {
    errors.rpcUrl = 'Enter a valid HTTPS RPC URL.';
  }

  if (draft.neighbors.length === 0) {
    errors.neighbors = 'Add at least one MASQ entry node.';
  } else {
    const invalid = draft.neighbors.find(
      descriptor => !isDescriptorForChain(descriptor, draft.chain),
    );
    if (invalid) {
      errors.neighbors = `Invalid ${draft.chain} node descriptor.`;
    }
  }

  if (
    !Number.isInteger(draft.minHops) ||
    draft.minHops < 1 ||
    draft.minHops > 6
  ) {
    errors.minHops = 'Choose between one and six MASQ hops.';
  }

  if (draft.exitCountry !== null && !/^[A-Z]{2}$/.test(draft.exitCountry)) {
    errors.exitCountry = 'Choose a valid two-letter exit country.';
  }

  const walletSecret = normalizeWalletSecret(draft);
  if (!walletSecret && options.walletRequired === false) {
    return errors;
  }
  if (draft.walletImportMode === 'seedPhrase') {
    const words = walletSecret ? walletSecret.split(' ') : [];
    if (words.length !== 12) {
      errors.walletSecret = 'Enter exactly 12 recovery words.';
    } else if (words.some(word => !/^[a-z]+$/.test(word))) {
      errors.walletSecret = 'Recovery words may only contain letters.';
    }
  } else if (!PRIVATE_KEY_PATTERN.test(walletSecret)) {
    errors.walletSecret =
      'A private key contains exactly 64 hexadecimal characters.';
  }

  return errors;
}

export function isValidSavedConfig(config: MasqConfig): boolean {
  if (
    config.configVersion !== 2 ||
    config.rpcUrl !== config.rpcUrl.trim() ||
    config.neighbors.some(node => node !== node.trim())
  ) {
    return false;
  }
  const draft: SetupDraft = {
    ...config,
    walletImportMode: 'seedPhrase',
    walletSecret: '',
  };
  return Object.keys(
    validateConfig(draft, { walletRequired: false }),
  ).length === 0;
}

export function normalizeWalletSecret(draft: SetupDraft): string {
  if (draft.walletImportMode === 'seedPhrase') {
    return draft.walletSecret
      .trim()
      .toLowerCase()
      .split(/\s+/)
      .filter(Boolean)
      .join(' ');
  }
  return draft.walletSecret.trim();
}

export function isDescriptorForChain(
  descriptor: string,
  chain: Chain,
): boolean {
  const portSeparator = descriptor.lastIndexOf(':');
  if (portSeparator < 0) {
    return false;
  }
  const ports = descriptor.slice(portSeparator + 1).split('/');
  const match = `${descriptor.slice(0, portSeparator + 1)}${ports[0]}`.match(
    DESCRIPTOR_PATTERN,
  );
  if (!match || match[1] !== chain) {
    return false;
  }
  return ports.map(Number).every(port => port > 0 && port <= 65535);
}

export function toMasqConfig(draft: SetupDraft): MasqConfig {
  return {
    configVersion: 2,
    chain: draft.chain,
    rpcUrl: draft.rpcUrl.trim(),
    neighbors: draft.neighbors,
    minHops: draft.minHops,
    exitCountry: draft.exitCountry,
    exitCountryFallback: draft.exitCountryFallback,
  };
}

export function normalizeBrowserUrl(input: string): string {
  const candidate = input.trim();
  if (!candidate) {
    throw new Error('Enter a web address.');
  }

  const withScheme = /^[a-zA-Z][a-zA-Z\d+.-]*:/.test(candidate)
    ? candidate
    : `https://${candidate}`;
  const url = new URL(withScheme);
  if (url.protocol !== 'https:') {
    throw new Error('MASQ Mobile only allows HTTPS addresses.');
  }
  if (isLocalHostname(url.hostname)) {
    throw new Error('Local addresses cannot be opened through MASQ.');
  }
  return url.toString();
}

function isLocalHostname(hostname: string): boolean {
  const normalized = hostname.replace(/^\[|\]$/g, '').toLowerCase();
  if (
    normalized === 'localhost' ||
    normalized === '0:0:0:0:0:0:0:0' ||
    normalized === '::1' ||
    normalized.endsWith('.local')
  ) {
    return true;
  }

  if (normalized.includes(':')) {
    const first = normalized.split(':', 1)[0];
    return (
      first.startsWith('fc') ||
      first.startsWith('fd') ||
      ['fe8', 'fe9', 'fea', 'feb'].some(prefix => first.startsWith(prefix))
    );
  }

  const octets = normalized.split('.').map(Number);
  if (octets.length !== 4 || octets.some(Number.isNaN)) {
    return false;
  }
  return (
    octets[0] === 0 ||
    octets[0] === 10 ||
    octets[0] === 127 ||
    (octets[0] === 100 && octets[1] >= 64 && octets[1] <= 127) ||
    (octets[0] === 169 && octets[1] === 254) ||
    (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31) ||
    (octets[0] === 192 && octets[1] === 168) ||
    octets[0] >= 224
  );
}
