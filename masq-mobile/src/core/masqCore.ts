import { Platform } from 'react-native';

import NativeMasqCore from '../../specs/NativeMasqCore';
import {
  decodeBrowserProtectionConfiguration,
  encodeBrowserProtectionPreferences,
  FALLBACK_BROWSER_PROTECTION_CONFIGURATION,
  type BrowserProtectionConfiguration,
  type BrowserProtectionPreferences,
} from './browserProtection';
import {
  decodeBrowserSiteSettings,
  type BrowserSiteMode,
  type BrowserSiteSettings,
} from './browserSiteSettings';
import {
  EMPTY_STATUS,
  type CoreStatus,
  type MasqConfig,
  type NetworkStatus,
} from './types';
import {
  decodeRoutableApps,
  decodeSystemTunnelStatus,
  type RoutableApp,
  type SystemTunnelMode,
  type SystemTunnelStatus,
} from './systemTunnel';

export type BrowserRoutingMode = 'blocked' | 'masq' | 'direct';

export interface MasqCore {
  getStatus(): Promise<CoreStatus>;
  getNetworkStatus(): Promise<NetworkStatus>;
  getNodeFinderUrl(): Promise<string>;
  getSavedConfiguration(): Promise<MasqConfig | null>;
  configure(config: MasqConfig): Promise<CoreStatus>;
  importWallet(privateKey: string): Promise<CoreStatus>;
  updateMinHops(minHops: number): Promise<CoreStatus>;
  start(): Promise<CoreStatus>;
  stop(): Promise<CoreStatus>;
  shutdown(): Promise<CoreStatus>;
  reset(): Promise<CoreStatus>;
  resetNetworkProfile(): Promise<CoreStatus>;
  removeWallet(): Promise<CoreStatus>;
  preflightBrowserProxy(): Promise<CoreStatus>;
  getSystemTunnelStatus(): Promise<SystemTunnelStatus>;
  getRoutableApps(): Promise<RoutableApp[]>;
  setSystemTunnel(
    mode: SystemTunnelMode,
    selectedApps: string[],
  ): Promise<SystemTunnelStatus>;
  prepareBrowserProtection(): Promise<BrowserProtectionConfiguration>;
  setBrowserProtection(
    preferences: BrowserProtectionPreferences,
  ): Promise<BrowserProtectionConfiguration>;
  setBrowserRoutingMode(mode: BrowserRoutingMode): Promise<BrowserRoutingMode>;
  getBrowserSiteSettings(
    mode: BrowserSiteMode,
    hostname: string,
  ): Promise<BrowserSiteSettings>;
  setBrowserSiteSettings(
    mode: BrowserSiteMode,
    hostname: string,
    rememberSignIn: boolean,
    protectionDisabled: boolean,
  ): Promise<BrowserSiteSettings>;
  clearBrowserSiteData(
    mode: BrowserSiteMode,
    hostname: string,
  ): Promise<BrowserSiteSettings>;
  clearRememberedBrowserData(): Promise<void>;
}

class NativeCore implements MasqCore {
  async getStatus(): Promise<CoreStatus> {
    return decodeStatus(await NativeMasqCore!.getStatus());
  }

  async getNetworkStatus(): Promise<NetworkStatus> {
    return decodeNetworkStatus(await NativeMasqCore!.getNetworkStatus());
  }

  getNodeFinderUrl(): Promise<string> {
    return NativeMasqCore!.getNodeFinderUrl();
  }

  async getSavedConfiguration(): Promise<MasqConfig | null> {
    const serialized = await NativeMasqCore!.getSavedConfiguration();
    if (serialized === 'null') {
      return null;
    }
    const parsed: unknown = JSON.parse(serialized);
    if (!isSavedConfig(parsed)) {
      throw new Error('The saved MASQ configuration is invalid.');
    }
    return parsed;
  }

  async configure(config: MasqConfig): Promise<CoreStatus> {
    return decodeOperationStatus(
      await NativeMasqCore!.configure(JSON.stringify(config)),
    );
  }

  async importWallet(privateKey: string): Promise<CoreStatus> {
    return decodeOperationStatus(
      await NativeMasqCore!.importWallet(privateKey),
    );
  }

  async updateMinHops(minHops: number): Promise<CoreStatus> {
    return decodeOperationStatus(await NativeMasqCore!.updateMinHops(minHops));
  }

  async start(): Promise<CoreStatus> {
    return decodeOperationStatus(await NativeMasqCore!.start());
  }

  async stop(): Promise<CoreStatus> {
    return decodeStatus(await NativeMasqCore!.stop());
  }

  async shutdown(): Promise<CoreStatus> {
    return decodeStatus(await NativeMasqCore!.shutdown());
  }

  async reset(): Promise<CoreStatus> {
    return decodeStatus(await NativeMasqCore!.reset());
  }

  async resetNetworkProfile(): Promise<CoreStatus> {
    return decodeStatus(await NativeMasqCore!.resetNetworkProfile());
  }

  async removeWallet(): Promise<CoreStatus> {
    return decodeStatus(await NativeMasqCore!.removeWallet());
  }

  async preflightBrowserProxy(): Promise<CoreStatus> {
    return decodeOperationStatus(await NativeMasqCore!.preflightBrowserProxy());
  }

  async getSystemTunnelStatus(): Promise<SystemTunnelStatus> {
    return decodeSystemTunnelStatus(
      await NativeMasqCore!.getSystemTunnelStatus(),
    );
  }

  async getRoutableApps(): Promise<RoutableApp[]> {
    return decodeRoutableApps(await NativeMasqCore!.getRoutableApps());
  }

  async setSystemTunnel(
    mode: SystemTunnelMode,
    selectedApps: string[],
  ): Promise<SystemTunnelStatus> {
    return decodeSystemTunnelStatus(
      await NativeMasqCore!.setSystemTunnel(mode, JSON.stringify(selectedApps)),
    );
  }

  async prepareBrowserProtection(): Promise<BrowserProtectionConfiguration> {
    return decodeBrowserProtectionConfiguration(
      await NativeMasqCore!.prepareBrowserProtection(),
    );
  }

  async setBrowserProtection(
    preferences: BrowserProtectionPreferences,
  ): Promise<BrowserProtectionConfiguration> {
    return decodeBrowserProtectionConfiguration(
      await NativeMasqCore!.setBrowserProtection(
        encodeBrowserProtectionPreferences(preferences),
      ),
    );
  }

  async setBrowserRoutingMode(
    mode: BrowserRoutingMode,
  ): Promise<BrowserRoutingMode> {
    return decodeBrowserRoutingMode(
      await NativeMasqCore!.setBrowserRoutingMode(mode),
    );
  }

  async getBrowserSiteSettings(
    mode: BrowserSiteMode,
    hostname: string,
  ): Promise<BrowserSiteSettings> {
    return decodeBrowserSiteSettings(
      await NativeMasqCore!.getBrowserSiteSettings(mode, hostname),
    );
  }

  async setBrowserSiteSettings(
    mode: BrowserSiteMode,
    hostname: string,
    rememberSignIn: boolean,
    protectionDisabled: boolean,
  ): Promise<BrowserSiteSettings> {
    return decodeBrowserSiteSettings(
      await NativeMasqCore!.setBrowserSiteSettings(
        mode,
        hostname,
        rememberSignIn,
        protectionDisabled,
      ),
    );
  }

  async clearBrowserSiteData(
    mode: BrowserSiteMode,
    hostname: string,
  ): Promise<BrowserSiteSettings> {
    return decodeBrowserSiteSettings(
      await NativeMasqCore!.clearBrowserSiteData(mode, hostname),
    );
  }

  async clearRememberedBrowserData(): Promise<void> {
    const result = await NativeMasqCore!.clearRememberedBrowserData();
    if (result !== 'ok') {
      throw new Error('The native core did not confirm browser data deletion.');
    }
  }
}

class MissingNativeCore implements MasqCore {
  private status: CoreStatus = {
    ...EMPTY_STATUS,
    phase: 'blocked',
    lastError:
      Platform.OS === 'web'
        ? 'MASQ Mobile requires iOS or Android.'
        : 'The native MASQ core is not included in this build.',
  };

  async getStatus(): Promise<CoreStatus> {
    return this.status;
  }

  async getSavedConfiguration(): Promise<MasqConfig | null> {
    return null;
  }

  async getNetworkStatus(): Promise<NetworkStatus> {
    return {
      available: false,
      interface: 'unknown',
      expensive: false,
      constrained: false,
      generation: 0,
    };
  }

  async getNodeFinderUrl(): Promise<string> {
    throw new Error('The MASQ node-finder is unavailable in this build.');
  }

  async configure(config: MasqConfig): Promise<CoreStatus> {
    this.status = { ...this.status, chain: config.chain };
    return this.status;
  }

  async importWallet(): Promise<CoreStatus> {
    return this.status;
  }

  async updateMinHops(minHops: number): Promise<CoreStatus> {
    this.status = { ...this.status, minHops };
    return this.status;
  }

  async start(): Promise<CoreStatus> {
    return this.status;
  }

  async stop(): Promise<CoreStatus> {
    return this.status;
  }

  async shutdown(): Promise<CoreStatus> {
    return this.status;
  }

  async reset(): Promise<CoreStatus> {
    this.status = { ...EMPTY_STATUS };
    return this.status;
  }

  async resetNetworkProfile(): Promise<CoreStatus> {
    this.status = { ...this.status, chain: null, phase: 'unconfigured' };
    return this.status;
  }

  async removeWallet(): Promise<CoreStatus> {
    this.status = {
      ...this.status,
      walletAddress: null,
      phase: 'unconfigured',
    };
    return this.status;
  }

  async preflightBrowserProxy(): Promise<CoreStatus> {
    return this.status;
  }

  async getSystemTunnelStatus(): Promise<SystemTunnelStatus> {
    return {
      supported: false,
      active: false,
      mode: 'off',
      phase: 'off',
      selectedApps: [],
      lastError: null,
    };
  }

  async getRoutableApps(): Promise<RoutableApp[]> {
    return [];
  }

  async setSystemTunnel(): Promise<SystemTunnelStatus> {
    throw new Error('Whole-device routing is unavailable in this build.');
  }

  async prepareBrowserProtection(): Promise<BrowserProtectionConfiguration> {
    return { ...FALLBACK_BROWSER_PROTECTION_CONFIGURATION };
  }

  async setBrowserProtection(
    preferences: BrowserProtectionPreferences,
  ): Promise<BrowserProtectionConfiguration> {
    encodeBrowserProtectionPreferences(preferences);
    return {
      ...FALLBACK_BROWSER_PROTECTION_CONFIGURATION,
      blockAdsAndTrackers: preferences.blockAdsAndTrackers,
      blockCrossSiteCookies: preferences.blockCrossSiteCookies,
      hideCookieBanners: preferences.hideCookieBanners,
      rejectOptionalCookies: preferences.rejectOptionalCookies,
      youtubeBestEffort: false,
    };
  }

  async setBrowserRoutingMode(
    mode: BrowserRoutingMode,
  ): Promise<BrowserRoutingMode> {
    if (mode === 'masq') {
      throw new Error('The native MASQ core is not included in this build.');
    }
    return mode;
  }

  async getBrowserSiteSettings(
    mode: BrowserSiteMode,
    hostname: string,
  ): Promise<BrowserSiteSettings> {
    return {
      hostname,
      mode,
      persistentSessionsSupported: false,
      protectionDisabled: false,
      rememberSignIn: false,
    };
  }

  async setBrowserSiteSettings(
    mode: BrowserSiteMode,
    hostname: string,
    rememberSignIn: boolean,
    protectionDisabled: boolean,
  ): Promise<BrowserSiteSettings> {
    if (rememberSignIn) {
      throw new Error(
        'Isolated persistent browser profiles are unavailable in this build.',
      );
    }
    return {
      hostname,
      mode,
      persistentSessionsSupported: false,
      protectionDisabled,
      rememberSignIn: false,
    };
  }

  async clearBrowserSiteData(
    mode: BrowserSiteMode,
    hostname: string,
  ): Promise<BrowserSiteSettings> {
    return {
      hostname,
      mode,
      persistentSessionsSupported: false,
      protectionDisabled: false,
      rememberSignIn: false,
    };
  }

  async clearRememberedBrowserData(): Promise<void> {}
}

function decodeBrowserRoutingMode(serialized: string): BrowserRoutingMode {
  if (
    serialized !== 'blocked' &&
    serialized !== 'masq' &&
    serialized !== 'direct'
  ) {
    throw new Error(
      'The native core returned an invalid browser routing mode.',
    );
  }
  return serialized;
}

function decodeStatus(serialized: string): CoreStatus {
  const parsed: unknown = JSON.parse(serialized);
  if (!isCoreStatus(parsed)) {
    throw new Error('The native core returned an invalid status response.');
  }
  return parsed;
}

function decodeOperationStatus(serialized: string): CoreStatus {
  const status = decodeStatus(serialized);
  if (status.phase === 'error') {
    throw new Error(
      status.lastError || 'The MASQ core rejected the operation.',
    );
  }
  return status;
}

function isCoreStatus(value: unknown): value is CoreStatus {
  if (!value || typeof value !== 'object') {
    return false;
  }
  const status = value as Partial<CoreStatus>;
  return (
    typeof status.phase === 'string' &&
    typeof status.engineAvailable === 'boolean' &&
    typeof status.proxyEnabled === 'boolean' &&
    (typeof status.proxyPort === 'number' || status.proxyPort === null) &&
    (typeof status.chain === 'string' || status.chain === null) &&
    (typeof status.walletAddress === 'string' ||
      status.walletAddress === null) &&
    typeof status.connectedNeighbors === 'number' &&
    typeof status.routeStage === 'number' &&
    typeof status.routeHops === 'number' &&
    typeof status.minHops === 'number' &&
    (typeof status.exitCountry === 'string' || status.exitCountry === null) &&
    typeof status.exitCountryFallback === 'boolean' &&
    Array.isArray(status.availableExitCountries) &&
    status.availableExitCountries.every(
      country => typeof country === 'string' && /^[A-Z]{2}$/.test(country),
    ) &&
    typeof status.bytesUp === 'number' &&
    typeof status.bytesDown === 'number' &&
    (typeof status.lastError === 'string' || status.lastError === null)
  );
}

function isSavedConfig(value: unknown): value is MasqConfig {
  if (!value || typeof value !== 'object') {
    return false;
  }
  const config = value as Partial<MasqConfig>;
  return (
    typeof config.configVersion === 'number' &&
    (config.chain === 'base-mainnet' || config.chain === 'base-sepolia') &&
    typeof config.rpcUrl === 'string' &&
    Array.isArray(config.neighbors) &&
    config.neighbors.every(node => typeof node === 'string') &&
    typeof config.minHops === 'number' &&
    (typeof config.exitCountry === 'string' || config.exitCountry === null) &&
    typeof config.exitCountryFallback === 'boolean'
  );
}

function decodeNetworkStatus(serialized: string): NetworkStatus {
  const parsed: unknown = JSON.parse(serialized);
  if (!parsed || typeof parsed !== 'object') {
    throw new Error('The native network monitor returned invalid data.');
  }
  const status = parsed as Partial<NetworkStatus>;
  if (
    typeof status.available !== 'boolean' ||
    !['wifi', 'cellular', 'wired', 'other', 'unknown'].includes(
      status.interface || '',
    ) ||
    typeof status.expensive !== 'boolean' ||
    typeof status.constrained !== 'boolean' ||
    typeof status.generation !== 'number'
  ) {
    throw new Error('The native network monitor returned invalid data.');
  }
  return status as NetworkStatus;
}

export const masqCore: MasqCore = NativeMasqCore
  ? new NativeCore()
  : new MissingNativeCore();
