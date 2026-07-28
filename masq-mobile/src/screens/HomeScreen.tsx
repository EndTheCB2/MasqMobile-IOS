import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import type { CoreStatus, NetworkStatus } from '../core/types';
import type { MasqIssue } from '../core/issues';
import type { WalletBalanceState } from '../core/walletBalance';
import {
  SYSTEM_TUNNEL_PUBLICLY_ENABLED,
  type SystemTunnelStatus,
} from '../core/systemTunnel';
import { HOP_OPTIONS, exitCountryName } from '../core/routingPreferences';
import {
  BrandMark,
  Button,
  Card,
  ErrorBanner,
  ScreenHeader,
} from '../ui/components';
import { colors, radii } from '../ui/theme';

interface Props {
  status: CoreStatus;
  busy: boolean;
  profileReady: boolean;
  initializationState: 'loading' | 'ready' | 'error';
  network: NetworkStatus;
  connectionProgress: { step: number; total: number; label: string };
  entryNodeRefresh: { attempt: number; maxAttempts: number } | null;
  issue: MasqIssue | null;
  walletBalance: WalletBalanceState;
  systemTunnel: SystemTunnelStatus;
  onConnect: () => void;
  onRetryInitialization: () => void;
  onDisconnect: () => void;
  onReset: () => void;
  onResetNetwork: () => void;
  onRemoveWallet: () => void;
  onRetry: () => void;
  onOpenSystemSettings: () => void;
  onShareDiagnostics: () => void;
  onOpenBrowser: () => void;
  onOpenDirectBrowser: () => void;
  onOpenSetup: () => void;
  onOpenTrafficRouting: () => void;
  onOpenPrivacy: () => void;
  onUpdateMinHops: (minHops: number) => void;
  onRefreshWalletBalance: () => void;
}

export function HomeScreen({
  status,
  busy,
  profileReady,
  initializationState,
  network,
  connectionProgress,
  entryNodeRefresh,
  issue,
  walletBalance,
  systemTunnel,
  onConnect,
  onRetryInitialization,
  onDisconnect,
  onReset,
  onResetNetwork,
  onRemoveWallet,
  onRetry,
  onOpenSystemSettings,
  onShareDiagnostics,
  onOpenBrowser,
  onOpenDirectBrowser,
  onOpenSetup,
  onOpenTrafficRouting,
  onOpenPrivacy,
  onUpdateMinHops,
  onRefreshWalletBalance,
}: Props) {
  const connected = profileReady && status.phase === 'connected';
  const configured = Boolean(
    profileReady && status.chain && status.walletAddress,
  );

  return (
    <View style={styles.screen}>
      <ScreenHeader title="MASQ" />
      <ScrollView
        contentContainerStyle={styles.content}
        showsVerticalScrollIndicator={false}
      >
        <ErrorBanner message={issue?.message ?? null} />
        {issue ? (
          <View style={styles.errorActions}>
            {issue.action === 'retry' ? (
              <Pressable
                accessibilityRole="button"
                accessibilityState={{ disabled: !profileReady }}
                disabled={!profileReady}
                onPress={onRetry}
                style={[
                  styles.errorAction,
                  !profileReady && styles.profileActionDisabled,
                ]}
              >
                <Text style={styles.errorActionText}>Retry connection</Text>
              </Pressable>
            ) : null}
            {issue.action === 'settings' ? (
              <Pressable
                accessibilityRole="button"
                onPress={onOpenSystemSettings}
                style={styles.errorAction}
              >
                <Text style={styles.errorActionText}>Open device settings</Text>
              </Pressable>
            ) : null}
            {issue.action === 'network-profile' || issue.action === 'wallet' ? (
              <Pressable
                accessibilityRole="button"
                accessibilityState={{ disabled: !profileReady }}
                disabled={!profileReady}
                onPress={onOpenSetup}
                style={[
                  styles.errorAction,
                  !profileReady && styles.profileActionDisabled,
                ]}
              >
                <Text style={styles.errorActionText}>
                  {issue.action === 'wallet'
                    ? 'Review consumer wallet'
                    : 'Review network profile'}
                </Text>
              </Pressable>
            ) : null}
            <Pressable
              accessibilityRole="button"
              onPress={onShareDiagnostics}
              style={styles.errorAction}
            >
              <Text style={styles.errorActionText}>
                Share redacted diagnostics
              </Text>
            </Pressable>
          </View>
        ) : null}

        <View style={[styles.hero, connected && styles.heroConnected]}>
          <View style={styles.heroGlow} />
          <View style={styles.logoShell}>
            <BrandMark large />
          </View>
          <View style={styles.badge}>
            <View
              style={[styles.badgeDot, connected && styles.badgeDotConnected]}
            />
            <Text style={styles.badgeText}>MASQ DMESH · CONSUME</Text>
          </View>
          <Text style={styles.state}>
            {profileReady
              ? phaseLabel(status)
              : initializationState === 'loading'
              ? 'Loading saved profile…'
              : 'Saved profile unavailable'}
          </Text>
          <Text style={styles.subtitle}>
            {!profileReady
              ? initializationState === 'loading'
                ? 'Node and wallet actions stay locked until the complete saved profile is available.'
                : 'Retry profile loading before changing Node, wallet or routing settings.'
              : connected
              ? `${
                  status.routeHops
                    ? `${status.routeHops} ${
                        status.routeHops === 1 ? 'hop' : 'hops'
                      } ready`
                    : 'Entry node connected'
                } · ${status.connectedNeighbors} peer${
                  status.connectedNeighbors === 1 ? '' : 's'
                }${status.proxyEnabled ? ' · browser protected' : ''}`
              : status.phase === 'connecting'
              ? `Step ${connectionProgress.step} of ${connectionProgress.total} · ${connectionProgress.label}.`
              : 'MASQ Private traffic only loads when a private MASQ route is active.'}
          </Text>
          <Text style={styles.networkState}>
            {network.available
              ? `${
                  network.interface === 'cellular'
                    ? 'Mobile data'
                    : network.interface
                } available`
              : network.interface === 'unknown'
              ? 'Checking internet connection…'
              : 'Internet connection unavailable'}
          </Text>
        </View>

        <View style={styles.actions}>
          {!profileReady ? (
            <Button
              accessibilityLabel={
                initializationState === 'loading'
                  ? 'Loading saved Node and wallet profile'
                  : 'Retry saved profile loading'
              }
              accessibilityState={{
                busy: initializationState === 'loading',
                disabled: initializationState === 'loading',
              }}
              label={
                initializationState === 'loading'
                  ? 'Loading saved profile…'
                  : 'Retry profile loading'
              }
              onPress={onRetryInitialization}
              busy={initializationState === 'loading'}
              disabled={initializationState === 'loading'}
            />
          ) : connected ? (
            <>
              <Button
                label={
                  status.routeHops > 0
                    ? 'Open private browser'
                    : 'Test & open private browser'
                }
                onPress={onOpenBrowser}
                busy={busy}
              />
              <Button
                label="Pause MASQ"
                onPress={onDisconnect}
                tone="secondary"
                busy={busy}
              />
            </>
          ) : (
            <>
              <Button
                label={
                  status.phase === 'connecting'
                    ? entryNodeRefresh
                      ? `Refreshing entry nodes · ${entryNodeRefresh.attempt}/${entryNodeRefresh.maxAttempts}`
                      : 'Contacting entry nodes…'
                    : configured
                    ? 'Connect to MASQ'
                    : 'Set up consumer wallet'
                }
                onPress={configured ? onConnect : onOpenSetup}
                busy={busy}
                disabled={
                  status.phase === 'connecting' ||
                  (status.phase === 'blocked' && configured)
                }
              />
              {status.phase === 'connecting' ? (
                <Button
                  label="Cancel connection"
                  onPress={onDisconnect}
                  tone="secondary"
                />
              ) : null}
            </>
          )}
          <Button
            label="Browse without MASQ"
            onPress={onOpenDirectBrowser}
            tone="secondary"
            disabled={
              !profileReady ||
              busy ||
              (!network.available && network.interface !== 'unknown')
            }
          />
        </View>

        <View style={styles.statsRow}>
          <Stat label="DOWNLOADED" value={formatBytes(status.bytesDown)} />
          <Stat label="UPLOADED" value={formatBytes(status.bytesUp)} />
        </View>

        {configured ? (
          <View style={styles.walletCard}>
            <Card>
              <View style={styles.walletHeader}>
                <View>
                  <Text style={styles.walletEyebrow}>CONSUMER FUNDS</Text>
                  <Text style={styles.walletTitle}>Wallet guardrails</Text>
                </View>
                <Pressable
                  accessibilityRole="button"
                  disabled={walletBalance.state === 'loading'}
                  onPress={onRefreshWalletBalance}
                  style={({ pressed }) => [
                    styles.walletRefresh,
                    pressed && styles.pressed,
                  ]}
                >
                  <Text style={styles.walletRefreshText}>
                    {walletBalance.state === 'loading'
                      ? 'Checking…'
                      : 'Refresh'}
                  </Text>
                </Pressable>
              </View>
              {walletBalance.value ? (
                <>
                  <View style={styles.walletBalances}>
                    <Stat
                      label="MASQ"
                      value={walletBalance.value.masqBalance}
                    />
                    <Stat
                      label="BASE ETH"
                      value={walletBalance.value.gasBalance}
                    />
                  </View>
                  {walletBalance.value.lowMasq || walletBalance.value.lowGas ? (
                    <View style={styles.fundsWarning}>
                      <Text style={styles.fundsWarningText}>
                        {walletBalance.value.lowMasq
                          ? 'No MASQ is available for consuming paid routes. '
                          : ''}
                        {walletBalance.value.lowGas
                          ? `Add Base ETH for settlement fees. Recommended reserve at the current gas price: ${walletBalance.value.gasReserve} ETH.`
                          : ''}
                      </Text>
                    </View>
                  ) : (
                    <Text style={styles.walletHelper}>
                      Funding detected · current gas price{' '}
                      {walletBalance.value.gasPriceGwei} Gwei
                    </Text>
                  )}
                </>
              ) : (
                <Text style={styles.walletHelper}>
                  {walletBalance.state === 'loading'
                    ? 'Checking MASQ and Base ETH balances…'
                    : 'Refresh to check whether this wallet can fund and settle a route.'}
                </Text>
              )}
              {walletBalance.state === 'error' ? (
                <Text style={styles.balanceError}>{walletBalance.message}</Text>
              ) : null}
            </Card>
          </View>
        ) : null}

        {configured ? (
          <View style={styles.routeLengthCard}>
            <View style={styles.routeLengthHeader}>
              <View>
                <Text style={styles.routeLengthEyebrow}>ROUTE LENGTH</Text>
                <Text style={styles.routeLengthTitle}>Minimum MASQ hops</Text>
              </View>
              <Text style={styles.routeLengthCurrent}>
                {status.minHops} {status.minHops === 1 ? 'hop' : 'hops'}
              </Text>
            </View>
            <View style={styles.hopSelector}>
              {HOP_OPTIONS.map(hops => {
                const selected = status.minHops === hops;
                return (
                  <Pressable
                    accessibilityLabel={`${hops} ${
                      hops === 1 ? 'hop' : 'hops'
                    }`}
                    accessibilityRole="radio"
                    accessibilityState={{ checked: selected, disabled: busy }}
                    disabled={busy}
                    key={hops}
                    onPress={() => onUpdateMinHops(hops)}
                    style={({ pressed }) => [
                      styles.hopOption,
                      selected && styles.hopOptionSelected,
                      pressed && styles.pressed,
                    ]}
                  >
                    <Text
                      style={[
                        styles.hopOptionText,
                        selected && styles.hopOptionTextSelected,
                      ]}
                    >
                      {hops}
                    </Text>
                  </Pressable>
                );
              })}
            </View>
            <Text style={styles.routeLengthHelper}>
              Changes apply immediately. Your wallet, entry nodes and network
              profile stay unchanged.
            </Text>
          </View>
        ) : null}

        {configured &&
        systemTunnel.supported &&
        (SYSTEM_TUNNEL_PUBLICLY_ENABLED || systemTunnel.phase !== 'off') ? (
          <Pressable
            accessibilityRole="button"
            onPress={onOpenTrafficRouting}
            style={({ pressed }) => [
              styles.trafficRouting,
              pressed && styles.pressed,
            ]}
          >
            <Card>
              <View style={styles.settingsRow}>
                <View style={styles.settingsBody}>
                  <Text style={styles.settingsEyebrow}>TRAFFIC SCOPE</Text>
                  <Text style={styles.settingsTitle}>
                    {SYSTEM_TUNNEL_PUBLICLY_ENABLED
                      ? systemTunnelTitle(systemTunnel)
                      : 'Turn off experimental system routing'}
                  </Text>
                  <Text style={styles.settingsMeta}>
                    {SYSTEM_TUNNEL_PUBLICLY_ENABLED
                      ? 'Configure whole-device or per-app routing'
                      : 'New system tunnels are disabled in this preview'}
                  </Text>
                </View>
                <Text style={styles.chevron}>›</Text>
              </View>
            </Card>
          </Pressable>
        ) : null}

        <Pressable
          accessibilityState={{ disabled: !profileReady }}
          accessibilityRole="button"
          disabled={!profileReady}
          onPress={onOpenSetup}
          style={({ pressed }) => [
            styles.settings,
            !profileReady && styles.profileActionDisabled,
            pressed && styles.pressed,
          ]}
        >
          <Card>
            <View style={styles.settingsRow}>
              <View style={styles.settingsBody}>
                <Text style={styles.settingsEyebrow}>CONNECTION PROFILE</Text>
                <Text style={styles.settingsTitle}>Node & wallet settings</Text>
                <Text numberOfLines={1} style={styles.settingsMeta}>
                  {!profileReady
                    ? initializationState === 'loading'
                      ? 'Loading the complete saved profile…'
                      : 'Retry profile loading to edit safely'
                    : status.walletAddress
                    ? `${status.chain} · ${status.minHops} ${
                        status.minHops === 1 ? 'hop' : 'hops'
                      } · ${exitCountryName(status.exitCountry)}`
                    : 'Not configured yet'}
                </Text>
                {status.walletAddress ? (
                  <Text numberOfLines={1} style={styles.walletMeta}>
                    {shortAddress(status.walletAddress)}
                  </Text>
                ) : null}
              </View>
              <Text style={styles.chevron}>›</Text>
            </View>
          </Card>
        </Pressable>

        <Pressable
          accessibilityRole="button"
          onPress={onOpenPrivacy}
          style={({ pressed }) => [styles.settings, pressed && styles.pressed]}
        >
          <Card>
            <View style={styles.settingsRow}>
              <View style={styles.settingsBody}>
                <Text style={styles.settingsEyebrow}>PRIVACY &amp; LEGAL</Text>
                <Text style={styles.settingsTitle}>How MASQ handles data</Text>
                <Text style={styles.settingsMeta}>
                  Local wallet storage, network services and licences
                </Text>
              </View>
              <Text style={styles.chevron}>›</Text>
            </View>
          </Card>
        </Pressable>

        {configured ? (
          <View style={styles.resetAction}>
            <Button
              label="Reset network profile"
              onPress={onResetNetwork}
              tone="danger"
              disabled={status.phase === 'connecting'}
            />
            <Text style={styles.resetHelper}>
              Keeps the saved consumer wallet and removes only connection
              settings.
            </Text>
            <Button
              label="Remove wallet from this device"
              onPress={onRemoveWallet}
              tone="danger"
              disabled={status.phase === 'connecting'}
            />
            <Button
              label="Reset everything"
              onPress={onReset}
              tone="danger"
              disabled={status.phase === 'connecting'}
            />
          </View>
        ) : null}

        <Text style={styles.disclaimer}>
          {status.minHops < 3
            ? 'One- and two-hop routes trade route separation for availability. Choose three or more hops for stronger anonymity when the live mesh can supply them. '
            : 'This profile requires a multi-hop route and may take longer to connect on a sparse mesh. '}
          MASQ Private never falls back to a direct connection. Direct browsing
          is a separate mode you must explicitly choose.
        </Text>
      </ScrollView>
    </View>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <View style={styles.stat}>
      <Text style={styles.statLabel}>{label}</Text>
      <Text style={styles.statValue}>{value}</Text>
    </View>
  );
}

function phaseLabel(status: CoreStatus): string {
  switch (status.phase) {
    case 'connected':
      if (status.routeHops > 0) {
        return 'MASQ route ready';
      }
      return status.proxyEnabled
        ? 'Testing private route…'
        : 'Entry nodes connected';
    case 'connecting':
      return 'Building private route…';
    case 'paused':
      return 'MASQ paused';
    case 'ready':
      return 'Ready to connect';
    case 'blocked':
      return 'Core unavailable';
    case 'error':
      return 'Connection failed';
    default:
      return 'Not connected';
  }
}

function formatBytes(value: number): string {
  if (value < 1024) {
    return `${value} B`;
  }
  if (value < 1024 * 1024) {
    return `${(value / 1024).toFixed(1)} KB`;
  }
  return `${(value / (1024 * 1024)).toFixed(1)} MB`;
}

function shortAddress(address: string): string {
  return address.length > 14
    ? `${address.slice(0, 7)}…${address.slice(-5)}`
    : address;
}

function systemTunnelTitle(status: SystemTunnelStatus): string {
  if (status.phase === 'blocked') {
    return status.active
      ? 'System traffic blocked'
      : 'System routing unavailable';
  }
  if (status.mode === 'wholeDevice' && status.active) {
    return 'Whole device protected';
  }
  if (status.mode === 'selectedApps' && status.active) {
    return `${status.selectedApps.length} selected app${
      status.selectedApps.length === 1 ? '' : 's'
    } protected`;
  }
  return 'Private browser only';
}

const styles = StyleSheet.create({
  screen: { backgroundColor: colors.ink, flex: 1 },
  content: { paddingBottom: 36, paddingHorizontal: 20 },
  hero: {
    alignItems: 'center',
    backgroundColor: '#081827',
    borderColor: '#173650',
    borderRadius: 30,
    borderWidth: 1,
    marginBottom: 18,
    overflow: 'hidden',
    paddingBottom: 30,
    paddingHorizontal: 18,
    paddingTop: 34,
  },
  heroConnected: { borderColor: '#1C8B88' },
  heroGlow: {
    backgroundColor: '#087BD9',
    borderRadius: 140,
    height: 210,
    opacity: 0.13,
    position: 'absolute',
    top: -96,
    width: 280,
  },
  logoShell: {
    alignItems: 'center',
    backgroundColor: '#06243A',
    borderColor: '#1D74A8',
    borderRadius: 64,
    borderWidth: 1,
    height: 128,
    justifyContent: 'center',
    marginBottom: 22,
    shadowColor: colors.violet,
    shadowOpacity: 0.3,
    shadowRadius: 28,
    width: 128,
  },
  badge: {
    alignItems: 'center',
    backgroundColor: '#0D2638',
    borderRadius: radii.pill,
    flexDirection: 'row',
    gap: 7,
    marginBottom: 12,
    paddingHorizontal: 11,
    paddingVertical: 6,
  },
  badgeDot: {
    backgroundColor: colors.violet,
    borderRadius: 4,
    height: 7,
    width: 7,
  },
  badgeDotConnected: { backgroundColor: colors.mint },
  badgeText: {
    color: '#85C9F6',
    fontSize: 10,
    fontWeight: '900',
    letterSpacing: 1.1,
  },
  state: {
    color: colors.white,
    fontSize: 28,
    fontWeight: '800',
    letterSpacing: -0.7,
    textAlign: 'center',
  },
  subtitle: {
    color: colors.muted,
    fontSize: 14,
    lineHeight: 21,
    marginTop: 8,
    textAlign: 'center',
  },
  networkState: {
    color: '#6F91A9',
    fontSize: 11,
    marginTop: 8,
    textTransform: 'capitalize',
  },
  errorActions: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 8,
    marginBottom: 16,
    marginTop: -8,
  },
  errorAction: {
    backgroundColor: '#17293A',
    borderColor: colors.line,
    borderRadius: 10,
    borderWidth: 1,
    paddingHorizontal: 11,
    paddingVertical: 9,
  },
  profileActionDisabled: { opacity: 0.48 },
  errorActionText: { color: colors.white, fontSize: 12, fontWeight: '700' },
  actions: { gap: 11 },
  statsRow: { flexDirection: 'row', gap: 12, marginTop: 22 },
  stat: {
    backgroundColor: colors.panel,
    borderColor: colors.line,
    borderRadius: radii.medium,
    borderWidth: 1,
    flex: 1,
    padding: 16,
  },
  statLabel: {
    color: colors.muted,
    fontSize: 10,
    fontWeight: '800',
    letterSpacing: 1,
    marginBottom: 8,
  },
  statValue: { color: colors.white, fontSize: 19, fontWeight: '800' },
  walletCard: { marginTop: 12 },
  walletHeader: {
    alignItems: 'center',
    flexDirection: 'row',
    justifyContent: 'space-between',
  },
  walletEyebrow: {
    color: colors.violet,
    fontSize: 10,
    fontWeight: '900',
    letterSpacing: 1.1,
  },
  walletTitle: {
    color: colors.white,
    fontSize: 16,
    fontWeight: '700',
    marginTop: 5,
  },
  walletRefresh: {
    backgroundColor: '#17293A',
    borderColor: colors.line,
    borderRadius: 10,
    borderWidth: 1,
    paddingHorizontal: 11,
    paddingVertical: 8,
  },
  walletRefreshText: { color: colors.white, fontSize: 11, fontWeight: '800' },
  walletBalances: { flexDirection: 'row', gap: 10, marginTop: 14 },
  walletHelper: {
    color: '#71879B',
    fontSize: 11,
    lineHeight: 16,
    marginTop: 11,
  },
  fundsWarning: {
    backgroundColor: '#392813',
    borderColor: '#805A1E',
    borderRadius: 10,
    borderWidth: 1,
    marginTop: 11,
    padding: 10,
  },
  fundsWarningText: { color: '#FFD48A', fontSize: 11, lineHeight: 16 },
  balanceError: { color: colors.red, fontSize: 11, marginTop: 8 },
  settings: { marginTop: 12 },
  trafficRouting: { marginTop: 12 },
  routeLengthCard: {
    backgroundColor: colors.panel,
    borderColor: colors.line,
    borderRadius: radii.medium,
    borderWidth: 1,
    marginTop: 12,
    padding: 16,
  },
  routeLengthHeader: {
    alignItems: 'center',
    flexDirection: 'row',
    justifyContent: 'space-between',
  },
  routeLengthEyebrow: {
    color: colors.violet,
    fontSize: 10,
    fontWeight: '900',
    letterSpacing: 1.1,
  },
  routeLengthTitle: {
    color: colors.white,
    fontSize: 16,
    fontWeight: '700',
    marginTop: 5,
  },
  routeLengthCurrent: {
    color: colors.mint,
    fontSize: 13,
    fontWeight: '800',
  },
  hopSelector: { flexDirection: 'row', gap: 7, marginTop: 15 },
  hopOption: {
    alignItems: 'center',
    backgroundColor: '#081522',
    borderColor: colors.line,
    borderRadius: 11,
    borderWidth: 1,
    flex: 1,
    justifyContent: 'center',
    minHeight: 42,
  },
  hopOptionSelected: {
    backgroundColor: '#173B67',
    borderColor: colors.violet,
  },
  hopOptionText: { color: colors.muted, fontSize: 15, fontWeight: '900' },
  hopOptionTextSelected: { color: colors.white },
  routeLengthHelper: {
    color: '#71879B',
    fontSize: 11,
    lineHeight: 16,
    marginTop: 10,
  },
  resetAction: { gap: 10, marginTop: 14 },
  resetHelper: {
    color: '#5E7487',
    fontSize: 11,
    lineHeight: 16,
    marginTop: 8,
    textAlign: 'center',
  },
  pressed: { opacity: 0.75 },
  settingsRow: {
    alignItems: 'center',
    flexDirection: 'row',
    justifyContent: 'space-between',
  },
  settingsBody: { flex: 1, paddingRight: 10 },
  settingsEyebrow: {
    color: colors.violet,
    fontSize: 10,
    fontWeight: '900',
    letterSpacing: 1.1,
  },
  settingsTitle: {
    color: colors.white,
    fontSize: 16,
    fontWeight: '700',
    marginTop: 5,
  },
  settingsMeta: { color: colors.muted, fontSize: 13, marginTop: 5 },
  walletMeta: { color: '#587086', fontSize: 11, marginTop: 3 },
  chevron: { color: colors.violet, fontSize: 32, fontWeight: '300' },
  disclaimer: {
    color: '#5E7487',
    fontSize: 12,
    lineHeight: 18,
    marginTop: 20,
    textAlign: 'center',
  },
});
