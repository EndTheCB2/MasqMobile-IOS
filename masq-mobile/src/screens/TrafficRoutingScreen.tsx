import { useEffect, useRef, useState } from 'react';
import {
  Alert,
  PermissionsAndroid,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import {
  appliedSystemTunnelScope,
  appliedSystemTunnelScopeKey,
  systemTunnelTrafficDisposition,
  type RoutableApp,
  type SystemTunnelMode,
  type SystemTunnelStatus,
} from '../core/systemTunnel';
import { Button, ErrorBanner, ScreenHeader } from '../ui/components';
import { colors, radii } from '../ui/theme';

interface Props {
  busy: boolean;
  connected: boolean;
  routableApps: RoutableApp[];
  status: SystemTunnelStatus;
  onApply: (mode: SystemTunnelMode, apps: string[]) => Promise<unknown>;
  onBack: () => void;
}

export function TrafficRoutingScreen({
  busy,
  connected,
  routableApps,
  status,
  onApply,
  onBack,
}: Props) {
  const appliedScope = appliedSystemTunnelScope(status);
  const appliedScopeKey = appliedSystemTunnelScopeKey(status);
  const trafficDisposition = systemTunnelTrafficDisposition(status);
  const observedAppliedScopeKey = useRef(appliedScopeKey);
  const [choice, setChoice] = useState<SystemTunnelMode>(appliedScope.mode);
  const [selectedApps, setSelectedApps] = useState(appliedScope.selectedApps);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (observedAppliedScopeKey.current === appliedScopeKey) {
      return;
    }
    observedAppliedScopeKey.current = appliedScopeKey;
    setChoice(appliedScope.mode);
    setSelectedApps(appliedScope.selectedApps);
  }, [appliedScope.mode, appliedScope.selectedApps, appliedScopeKey]);

  const apply = async (mode: SystemTunnelMode, apps: string[]) => {
    setError(null);
    try {
      if (
        mode !== 'off' &&
        Platform.OS === 'android' &&
        Number(Platform.Version) >= 33
      ) {
        const permission = PermissionsAndroid.PERMISSIONS.POST_NOTIFICATIONS;
        const alreadyGranted = await PermissionsAndroid.check(permission);
        const outcome = alreadyGranted
          ? PermissionsAndroid.RESULTS.GRANTED
          : await PermissionsAndroid.request(permission, {
              title: 'Allow the MASQ routing status notice',
              message:
                'Unsafe dogfood routing needs a visible ongoing notice so you can see when captured traffic is active, blocked, or may be direct.',
              buttonPositive: 'Allow notice',
              buttonNegative: 'Cancel',
            });
        if (outcome !== PermissionsAndroid.RESULTS.GRANTED) {
          throw new Error(
            'Allow notifications before starting dogfood system routing. Turning routing off remains available without this permission.',
          );
        }
      }
      await onApply(mode, apps);
    } catch (caught) {
      setError(
        caught instanceof Error
          ? caught.message
          : 'The system traffic scope could not be changed.',
      );
    }
  };

  const toggleApp = (id: string) => {
    setSelectedApps(current =>
      current.includes(id)
        ? current.filter(app => app !== id)
        : [...current, id],
    );
  };

  const confirmApply = (mode: SystemTunnelMode, apps: string[]) => {
    const stopping = mode === 'off';
    Alert.alert(
      stopping ? 'Turn off system routing?' : 'Confirm unsafe system routing',
      stopping
        ? 'MASQ will stop the experimental system route. Included apps will use their normal connection after native shutdown completes.'
        : 'This internal dogfood route is not a VPN safety guarantee. It sends IPv4 TCP connections to port 443 through MASQ and handles DNS virtually. All other captured IP traffic—including other TCP ports, non-DNS UDP, IPv6, ICMP, and unknown transports—stays blocked while capture is valid. Continue?',
      [
        { text: 'Cancel', style: 'cancel' },
        {
          text: stopping ? 'Turn off' : 'Apply dogfood route',
          style: stopping ? 'destructive' : 'default',
          onPress: () => {
            apply(mode, apps).catch(() => undefined);
          },
        },
      ],
    );
  };

  const unavailableReason =
    Platform.OS === 'ios'
      ? 'This iOS build has no signed Packet Tunnel entitlement. The private browser remains available and fail-closed.'
      : 'System routing is disabled or native packet-tunnel support is unavailable in this build.';

  return (
    <View style={styles.screen}>
      <ScreenHeader title="Traffic routing" onBack={onBack} />
      <ScrollView
        contentContainerStyle={styles.content}
        showsVerticalScrollIndicator={false}
      >
        <Text style={styles.eyebrow}>DOGFOOD SCOPE</Text>
        <Text style={styles.title}>Choose eligible traffic</Text>
        <Text style={styles.intro}>
          Configure the experimental Android system route exposed by this native
          build.
        </Text>
        <ErrorBanner message={error || status.lastError} />

        {!status.supported ? (
          <View style={styles.warning}>
            <Text style={styles.warningTitle}>System tunnel unavailable</Text>
            <Text style={styles.warningText}>{unavailableReason}</Text>
          </View>
        ) : null}

        {status.supported ? (
          <View style={styles.dogfoodWarning}>
            <Text style={styles.dogfoodWarningTitle}>
              Unsafe internal dogfood
            </Text>
            <Text style={styles.dogfoodWarningText}>
              Only IPv4 TCP connections to port 443 are sent through MASQ. DNS
              is handled virtually. All other captured IP traffic—including
              other TCP ports, non-DNS UDP, IPv6, ICMP, and unknown
              transports—is blocked while capture is valid. Activation opens
              a real CONNECT tunnel to example.com:443 through the MASQ exit
              to test reachability, without requesting a page or body. MASQ
              packages installed when the route is created are excluded, but
              Android snapshots package UIDs at that moment. Turn routing off
              before installing, removing, enabling, disabling, or updating
              apps, then reapply it. If the service or process dies, traffic
              can return to the direct connection. Always-on VPN and “Block
              connections without VPN” are unsupported. The loopback proxy is
              unauthenticated; a malicious local app that discovers its
              temporary port could consume the MASQ route and wallet funds.
              Android 13 or later must allow notifications before activation
              so the ongoing unsafe-routing state remains visible.
            </Text>
          </View>
        ) : null}

        {status.supported && trafficDisposition === 'directRisk' ? (
          <View style={styles.riskStatus}>
            <Text style={styles.riskStatusTitle}>Traffic may be direct</Text>
            <Text style={styles.riskStatusText}>
              Android cannot confirm that the requested app scope is captured.
              Do not assume MASQ routing. Use “Turn off system routing” below
              before continuing.
            </Text>
          </View>
        ) : null}

        {status.supported && trafficDisposition === 'blocked' ? (
          <View style={styles.blockedStatus}>
            <Text style={styles.blockedStatusTitle}>
              Captured traffic is blocked
            </Text>
            <Text style={styles.blockedStatusText}>
              Android reports that captured traffic cannot currently use MASQ
              and is being held instead of sent through the normal connection.
              Turn the route off to restore normal networking.
            </Text>
          </View>
        ) : null}

        {status.supported ? (
          <>
            <ScopeCard
              active={choice === 'off'}
              detail="Only the isolated MASQ browser uses MASQ. Other apps keep their normal connection."
              label="Private browser only"
              onPress={() => setChoice('off')}
            />
            <ScopeCard
              active={choice === 'wholeDevice'}
              detail="Capture Android app traffic; route only IPv4 TCP/443 and virtual DNS. Other traffic is blocked while capture remains valid."
              disabled={status.phase !== 'off'}
              label="Whole device"
              onPress={() => setChoice('wholeDevice')}
            />
            <ScopeCard
              active={choice === 'selectedApps'}
              detail="Capture selected app UIDs; route only IPv4 TCP/443 and virtual DNS. Other traffic is blocked while capture remains valid."
              disabled={status.phase !== 'off'}
              label="Selected apps"
              onPress={() => setChoice('selectedApps')}
            />
          </>
        ) : null}

        {status.supported &&
        choice === 'selectedApps' &&
        status.phase === 'off' ? (
          <View style={styles.appsCard}>
            <Text style={styles.appsTitle}>Apps in this dogfood route</Text>
            <Text style={styles.appsHelper}>
              Only launchable apps are listed. Package IDs and the consent
              timestamp stay on this device. Android applies VPN rules to UIDs:
              apps sharing a UID can share routing, attached restricted
              profiles may also receive the scope, and work-profile copies are
              a separate user scope. Package-to-UID rules are captured only
              when the route is created; turn routing off before app package
              changes, then reapply it.
            </Text>
            {routableApps.length === 0 ? (
              <Text style={styles.warningText}>
                No routable Android apps were reported by this build.
              </Text>
            ) : null}
            {routableApps.map(app => {
              const selected = selectedApps.includes(app.id);
              return (
                <Pressable
                  accessibilityRole="checkbox"
                  accessibilityState={{ checked: selected }}
                  key={app.id}
                  onPress={() => toggleApp(app.id)}
                  style={styles.appRow}
                >
                  <View
                    style={[styles.checkbox, selected && styles.checkboxOn]}
                  >
                    <Text style={styles.checkmark}>{selected ? '✓' : ''}</Text>
                  </View>
                  <View style={styles.appBody}>
                    <Text style={styles.appLabel}>{app.label}</Text>
                    <Text numberOfLines={1} style={styles.appId}>
                      {app.id}
                    </Text>
                  </View>
                </Pressable>
              );
            })}
          </View>
        ) : null}

        {status.supported && status.phase === 'off' && choice !== 'off' ? (
          <Button
            busy={busy}
            disabled={
              busy ||
              !connected ||
              (choice === 'selectedApps' && selectedApps.length === 0)
            }
            label={
              connected
                ? choice === 'wholeDevice'
                  ? 'Apply whole-device dogfood'
                  : 'Apply selected-app dogfood'
                : 'Connect MASQ first'
            }
            onPress={() =>
              confirmApply(
                choice,
                choice === 'selectedApps' ? selectedApps : [],
              )
            }
          />
        ) : status.supported && status.phase !== 'off' ? (
          <View style={styles.routeActions}>
            {status.mode !== 'off' &&
            status.phase !== 'stopping' &&
            trafficDisposition !== 'masq' ? (
              <Button
                busy={busy}
                disabled={busy || !connected}
                label={connected ? 'Retry MASQ route' : 'Connect MASQ first'}
                onPress={() =>
                  apply(
                    status.mode,
                    status.mode === 'selectedApps' ? status.selectedApps : [],
                  )
                }
              />
            ) : null}
            <Button
              busy={busy}
              disabled={busy || status.phase === 'stopping'}
              label={
                status.phase === 'stopping'
                  ? 'Stopping system routing…'
                  : 'Turn off system routing'
              }
              onPress={() => confirmApply('off', [])}
              tone="danger"
            />
          </View>
        ) : null}
      </ScrollView>
    </View>
  );
}

function ScopeCard({
  active,
  detail,
  disabled = false,
  label,
  onPress,
}: {
  active: boolean;
  detail: string;
  disabled?: boolean;
  label: string;
  onPress: () => void;
}) {
  return (
    <Pressable
      accessibilityRole="radio"
      accessibilityState={{ checked: active, disabled }}
      disabled={disabled}
      onPress={onPress}
      style={[
        styles.scope,
        active && styles.scopeActive,
        disabled && styles.disabled,
      ]}
    >
      <View style={[styles.radio, active && styles.radioActive]}>
        {active ? <View style={styles.radioCenter} /> : null}
      </View>
      <View style={styles.scopeBody}>
        <Text style={styles.scopeLabel}>{label}</Text>
        <Text style={styles.scopeDetail}>{detail}</Text>
      </View>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  screen: { backgroundColor: colors.ink, flex: 1 },
  content: { paddingBottom: 40, paddingHorizontal: 20, paddingTop: 12 },
  eyebrow: {
    color: colors.violet,
    fontSize: 11,
    fontWeight: '900',
    letterSpacing: 1.5,
  },
  title: { color: colors.white, fontSize: 29, fontWeight: '800', marginTop: 8 },
  intro: {
    color: colors.muted,
    fontSize: 13,
    lineHeight: 20,
    marginBottom: 20,
    marginTop: 9,
  },
  warning: {
    backgroundColor: '#392813',
    borderColor: '#805A1E',
    borderRadius: radii.medium,
    borderWidth: 1,
    marginBottom: 12,
    padding: 14,
  },
  warningTitle: { color: colors.amber, fontSize: 14, fontWeight: '800' },
  warningText: { color: '#DDBD84', fontSize: 12, lineHeight: 18, marginTop: 5 },
  dogfoodWarning: {
    backgroundColor: '#3A1518',
    borderColor: '#9D343D',
    borderRadius: radii.medium,
    borderWidth: 1,
    marginBottom: 14,
    padding: 14,
  },
  dogfoodWarningTitle: {
    color: '#FF8D95',
    fontSize: 14,
    fontWeight: '900',
  },
  dogfoodWarningText: {
    color: '#F2C3C6',
    fontSize: 12,
    lineHeight: 19,
    marginTop: 6,
  },
  riskStatus: {
    backgroundColor: '#49171B',
    borderColor: '#D34B56',
    borderRadius: radii.medium,
    borderWidth: 2,
    marginBottom: 14,
    padding: 14,
  },
  riskStatusTitle: { color: '#FF9DA4', fontSize: 15, fontWeight: '900' },
  riskStatusText: {
    color: '#FFD8DB',
    fontSize: 12,
    lineHeight: 19,
    marginTop: 6,
  },
  blockedStatus: {
    backgroundColor: '#392813',
    borderColor: '#B47C23',
    borderRadius: radii.medium,
    borderWidth: 1,
    marginBottom: 14,
    padding: 14,
  },
  blockedStatusTitle: { color: colors.amber, fontSize: 14, fontWeight: '900' },
  blockedStatusText: {
    color: '#EBD2A3',
    fontSize: 12,
    lineHeight: 19,
    marginTop: 6,
  },
  scope: {
    alignItems: 'flex-start',
    backgroundColor: colors.panel,
    borderColor: colors.line,
    borderRadius: radii.medium,
    borderWidth: 1,
    flexDirection: 'row',
    marginBottom: 10,
    padding: 15,
  },
  scopeActive: { borderColor: colors.violet },
  disabled: { opacity: 0.52 },
  radio: {
    alignItems: 'center',
    borderColor: colors.muted,
    borderRadius: 10,
    borderWidth: 1,
    height: 20,
    justifyContent: 'center',
    marginRight: 11,
    marginTop: 1,
    width: 20,
  },
  radioActive: { borderColor: colors.violet },
  radioCenter: {
    backgroundColor: colors.violet,
    borderRadius: 5,
    height: 10,
    width: 10,
  },
  scopeBody: { flex: 1 },
  scopeLabel: { color: colors.white, fontSize: 15, fontWeight: '800' },
  scopeDetail: {
    color: colors.muted,
    fontSize: 11,
    lineHeight: 17,
    marginTop: 4,
  },
  appsCard: {
    backgroundColor: '#081522',
    borderColor: colors.line,
    borderRadius: radii.medium,
    borderWidth: 1,
    marginBottom: 14,
    padding: 14,
  },
  routeActions: { gap: 10 },
  appsTitle: { color: colors.white, fontSize: 15, fontWeight: '800' },
  appsHelper: {
    color: colors.muted,
    fontSize: 11,
    marginBottom: 8,
    marginTop: 4,
  },
  appRow: {
    alignItems: 'center',
    borderTopColor: colors.line,
    borderTopWidth: 1,
    flexDirection: 'row',
    minHeight: 58,
    paddingVertical: 8,
  },
  checkbox: {
    alignItems: 'center',
    borderColor: colors.muted,
    borderRadius: 6,
    borderWidth: 1,
    height: 22,
    justifyContent: 'center',
    marginRight: 11,
    width: 22,
  },
  checkboxOn: { backgroundColor: colors.violet, borderColor: colors.violet },
  checkmark: { color: colors.white, fontSize: 13, fontWeight: '900' },
  appBody: { flex: 1 },
  appLabel: { color: colors.white, fontSize: 13, fontWeight: '700' },
  appId: { color: '#658096', fontSize: 10, marginTop: 3 },
});
