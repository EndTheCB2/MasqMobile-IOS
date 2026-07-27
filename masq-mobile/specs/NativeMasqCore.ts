import type { TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export interface Spec extends TurboModule {
  getStatus(): Promise<string>;
  getNetworkStatus(): Promise<string>;
  getNodeFinderUrl(): Promise<string>;
  getSavedConfiguration(): Promise<string>;
  configure(configJson: string): Promise<string>;
  importWallet(privateKey: string): Promise<string>;
  updateMinHops(minHops: number): Promise<string>;
  start(): Promise<string>;
  stop(): Promise<string>;
  shutdown(): Promise<string>;
  reset(): Promise<string>;
  resetNetworkProfile(): Promise<string>;
  removeWallet(): Promise<string>;
  preflightBrowserProxy(): Promise<string>;
  getSystemTunnelStatus(): Promise<string>;
  getRoutableApps(): Promise<string>;
  setSystemTunnel(mode: string, appIdsJson: string): Promise<string>;
  prepareBrowserProtection(): Promise<string>;
  setBrowserProtection(configJson: string): Promise<string>;
  setBrowserRoutingMode(mode: string): Promise<string>;
  getBrowserSiteSettings(mode: string, hostname: string): Promise<string>;
  setBrowserSiteSettings(
    mode: string,
    hostname: string,
    rememberSignIn: boolean,
    protectionDisabled: boolean,
  ): Promise<string>;
  clearBrowserSiteData(mode: string, hostname: string): Promise<string>;
  clearRememberedBrowserData(): Promise<string>;
}

export default TurboModuleRegistry.get<Spec>('NativeMasqCore');
