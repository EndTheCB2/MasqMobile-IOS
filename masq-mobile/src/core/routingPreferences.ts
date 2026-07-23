import { Platform } from 'react-native';

import type { ExitCountry } from './types';

export interface ExitCountryOption {
  code: ExitCountry;
  name: string;
}

export const EXIT_COUNTRIES: ExitCountryOption[] = [
  { code: null, name: 'Automatic (best available)' },
  { code: 'AU', name: 'Australia' },
  { code: 'AT', name: 'Austria' },
  { code: 'BE', name: 'Belgium' },
  { code: 'CA', name: 'Canada' },
  { code: 'CZ', name: 'Czechia' },
  { code: 'FR', name: 'France' },
  { code: 'DE', name: 'Germany' },
  { code: 'IE', name: 'Ireland' },
  { code: 'IT', name: 'Italy' },
  { code: 'JP', name: 'Japan' },
  { code: 'NL', name: 'Netherlands' },
  { code: 'PL', name: 'Poland' },
  { code: 'SG', name: 'Singapore' },
  { code: 'ES', name: 'Spain' },
  { code: 'SE', name: 'Sweden' },
  { code: 'CH', name: 'Switzerland' },
  { code: 'GB', name: 'United Kingdom' },
  { code: 'US', name: 'United States' },
];

export const HOP_OPTIONS = [1, 2, 3, 4, 5, 6] as const;

export type TrafficScope = 'privateBrowser' | 'wholeDevice' | 'selectedApps';

export interface TrafficScopeOption {
  id: TrafficScope;
  label: string;
  detail: string;
  available: boolean;
}

export const TRAFFIC_SCOPE_OPTIONS: TrafficScopeOption[] = [
  {
    id: 'privateBrowser',
    label: 'MASQ private browser',
    detail: 'Available now · fail-closed HTTPS traffic through MASQ.',
    available: true,
  },
  {
    id: 'wholeDevice',
    label: 'Whole device',
    detail:
      Platform.OS === 'ios'
        ? 'Requires a Packet Tunnel extension and Apple Network Extension entitlement.'
        : 'Requires an Android VpnService packet-tunnel integration.',
    available: false,
  },
  {
    id: 'selectedApps',
    label: 'Selected apps',
    detail:
      Platform.OS === 'ios'
        ? 'iOS only permits this for organisation-managed apps through MDM.'
        : 'Requires an Android VpnService per-app allowlist integration.',
    available: false,
  },
];

export function exitCountryName(country: ExitCountry): string {
  return (
    EXIT_COUNTRIES.find(option => option.code === country)?.name ??
    country ??
    'Automatic (best available)'
  );
}

export function exitCountryOptionsForAvailability(
  availableCountries: readonly string[],
  selectedCountry: ExitCountry,
  inventoryReady: boolean,
): ExitCountryOption[] {
  if (!inventoryReady) {
    return EXIT_COUNTRIES;
  }
  const available = new Set(
    availableCountries.filter(country => /^[A-Z]{2}$/.test(country)),
  );
  return EXIT_COUNTRIES.filter(
    option =>
      option.code === null ||
      option.code === selectedCountry ||
      available.has(option.code),
  );
}
