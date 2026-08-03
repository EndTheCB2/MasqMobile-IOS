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
  desiredSystemTunnelScope,
  desiredSystemTunnelScopeKey,
  systemTunnelTrafficDisposition,
  type RoutableApp,
  type SystemTunnelMode,
  type SystemTunnelStatus,
  type SystemTunnelTrafficDisposition,
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
  const desiredScope = desiredSystemTunnelScope(status);
  const desiredScopeKey = desiredSystemTunnelScopeKey(status);
  const trafficDisposition = systemTunnelTrafficDisposition(status);
  const observedDesiredScopeKey = useRef(desiredScopeKey);
  const draftDirty = useRef(false);
  const submittedDraft = useRef<{
    mode: SystemTunnelMode;
    selectedApps: string[];
  } | null>(null);
  const [choice, setChoice] = useState<SystemTunnelMode>(desiredScope.mode);
  const [selectedApps, setSelectedApps] = useState(
    desiredScope.selectedApps,
  );
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (observedDesiredScopeKey.current === desiredScopeKey) {
      return;
    }
    observedDesiredScopeKey.current = desiredScopeKey;
    const submitted = submittedDraft.current;
    if (
      draftDirty.current &&
      (!submitted ||
        !scopeMatchesDraft(
          desiredScope.mode,
          desiredScope.selectedApps,
          submitted,
        ))
    ) {
      return;
    }
    draftDirty.current = false;
    submittedDraft.current = null;
    setChoice(desiredScope.mode);
    setSelectedApps(desiredScope.selectedApps);
  }, [desiredScope.mode, desiredScope.selectedApps, desiredScopeKey]);

  const routingPhase = status.routingPhase || status.phase;
  const hasRoutingIntent =
    desiredScope.mode !== 'off' ||
    appliedScope.mode !== 'off' ||
    status.tunPresent === true ||
    routingPhase !== 'off';
  const editable = status.supported && !hasRoutingIntent;

  const apply = async (mode: SystemTunnelMode, apps: string[]) => {
    setError(null);
    draftDirty.current = true;
    submittedDraft.current = { mode, selectedApps: [...apps] };
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
                'Experimental community routing needs a visible ongoing notice so you can see when captured traffic is active, blocked, or may be direct.',
              buttonPositive: 'Allow notice',
              buttonNegative: 'Cancel',
            });
        if (outcome !== PermissionsAndroid.RESULTS.GRANTED) {
          throw new Error(
            'Allow notifications before starting community system routing. Turning routing off remains available without this permission.',
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
    draftDirty.current = true;
    submittedDraft.current = null;
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
        : 'This experimental community route is not a VPN safety guarantee. It sends IPv4 TCP connections to port 443 through MASQ and handles DNS virtually. All other captured IP traffic—including other TCP ports, non-DNS UDP, IPv6, ICMP, and unknown transports—stays blocked while capture is valid. Continue?',
      [
        { text: 'Cancel', style: 'cancel' },
        {
          text: stopping ? 'Turn off' : 'Apply community route',
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
        <Text style={styles.eyebrow}>COMMUNITY ROUTING SCOPE</Text>
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
              Experimental community routing
            </Text>
            <Text style={styles.dogfoodWarningText}>
              Only IPv4 TCP connections to port 443 are sent through MASQ. DNS
              is handled virtually. All other captured IP traffic—including
              other TCP ports, non-DNS UDP, IPv6, ICMP, and unknown
              transports—is blocked while capture is valid. Activation opens
              an encrypted connection to example.com through the MASQ exit
              and requests only response headers; no page body is downloaded. MASQ
              packages installed when the route is created are excluded, and
              Android snapshots package UIDs at that moment. When a scoped app
              is installed, removed, enabled, disabled, or updated, this build
              pauses translation and safely rebuilds that UID scope. Wait until
              the status returns to MASQ before using the affected app; turn
              routing off if Android cannot confirm the rebuilt scope. If the
              service or process dies, traffic can return to the direct
              connection. Always-on VPN and “Block
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

        {status.supported && hasRoutingIntent ? (
          <RoutingStatusCard
            appliedMode={appliedScope.mode}
            appliedSelectedApps={appliedScope.selectedApps}
            desiredMode={desiredScope.mode}
            desiredSelectedApps={desiredScope.selectedApps}
            disposition={trafficDisposition}
            routingPhase={routingPhase}
            trafficObserved={status.trafficObserved === true}
          />
        ) : null}

        {editable ? (
          <>
            <ScopeCard
              active={choice === 'off'}
              detail="Only the isolated MASQ browser uses MASQ. Other apps keep their normal connection."
              label="Private browser only"
              onPress={() => {
                draftDirty.current = true;
                submittedDraft.current = null;
                setChoice('off');
              }}
            />
            <ScopeCard
              active={choice === 'wholeDevice'}
              detail="Capture Android app traffic; route only IPv4 TCP/443 and virtual DNS. Other traffic is blocked while capture remains valid."
              label="Whole device"
              onPress={() => {
                draftDirty.current = true;
                submittedDraft.current = null;
                setChoice('wholeDevice');
              }}
            />
            <ScopeCard
              active={choice === 'selectedApps'}
              detail="Capture selected app UIDs; route only IPv4 TCP/443 and virtual DNS. Other traffic is blocked while capture remains valid."
              label="Selected apps"
              onPress={() => {
                draftDirty.current = true;
                submittedDraft.current = null;
                setChoice('selectedApps');
              }}
            />
          </>
        ) : null}

        {editable && choice === 'selectedApps' ? (
          <View style={styles.appsCard}>
            <Text style={styles.appsTitle}>Apps in this community route</Text>
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

        {editable && choice !== 'off' ? (
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
                  ? 'Apply whole-device routing'
                  : 'Apply selected-app routing'
                : 'Connect MASQ first'
            }
            onPress={() =>
              confirmApply(
                choice,
                choice === 'selectedApps' ? selectedApps : [],
              )
            }
          />
        ) : status.supported && hasRoutingIntent ? (
          <View style={styles.routeActions}>
            {desiredScope.mode !== 'off' &&
            (routingPhase === 'blocked' || routingPhase === 'revoked') &&
            trafficDisposition !== 'masq' ? (
              <Button
                busy={busy}
                disabled={busy || !connected}
                label={connected ? 'Retry MASQ route' : 'Connect MASQ first'}
                onPress={() =>
                  apply(
                    desiredScope.mode,
                    desiredScope.mode === 'selectedApps'
                      ? desiredScope.selectedApps
                      : [],
                  )
                }
              />
            ) : null}
            <Button
              busy={busy}
              disabled={busy || routingPhase === 'stopping'}
              label={
                routingPhase === 'stopping'
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

function RoutingStatusCard({
  appliedMode,
  appliedSelectedApps,
  desiredMode,
  desiredSelectedApps,
  disposition,
  routingPhase,
  trafficObserved,
}: {
  appliedMode: SystemTunnelMode;
  appliedSelectedApps: string[];
  desiredMode: SystemTunnelMode;
  desiredSelectedApps: string[];
  disposition: SystemTunnelTrafficDisposition;
  routingPhase: string;
  trafficObserved: boolean;
}) {
  const traffic =
    disposition === 'masq'
      ? trafficObserved
        ? 'Captured HTTPS session reached the local MASQ adapter'
        : 'MASQ route ready · waiting for compatible app traffic'
      : disposition === 'blocked'
        ? 'Captured traffic blocked'
        : disposition === 'directRisk'
          ? 'Capture not confirmed — traffic may be direct'
          : 'System routing off';
  return (
    <View style={styles.routingStatus}>
      <Text style={styles.routingStatusTitle}>System route status</Text>
      <RoutingStatusRow
        label="Requested"
        value={scopeDescription(desiredMode, desiredSelectedApps)}
      />
      <RoutingStatusRow
        label="Captured"
        value={scopeDescription(appliedMode, appliedSelectedApps)}
      />
      <RoutingStatusRow label="Traffic" value={traffic} />
      <RoutingStatusRow
        label="Phase"
        value={routingPhaseDescription(routingPhase, disposition)}
      />
    </View>
  );
}

function RoutingStatusRow({ label, value }: { label: string; value: string }) {
  return (
    <View style={styles.routingStatusRow}>
      <Text style={styles.routingStatusLabel}>{label}</Text>
      <Text style={styles.routingStatusValue}>{value}</Text>
    </View>
  );
}

function scopeDescription(mode: SystemTunnelMode, apps: string[]): string {
  if (mode === 'wholeDevice') return 'Whole device · compatible HTTPS only';
  if (mode === 'selectedApps') {
    return `${apps.length} selected app${apps.length === 1 ? '' : 's'}`;
  }
  return 'Not captured';
}

function scopeMatchesDraft(
  mode: SystemTunnelMode,
  selectedApps: string[],
  draft: { mode: SystemTunnelMode; selectedApps: string[] },
): boolean {
  if (mode !== draft.mode || selectedApps.length !== draft.selectedApps.length) {
    return false;
  }
  const expected = new Set(draft.selectedApps);
  return expected.size === selectedApps.length &&
    selectedApps.every(app => expected.has(app));
}

function routingPhaseDescription(
  phase: string,
  disposition: SystemTunnelTrafficDisposition,
): string {
  switch (phase) {
    case 'requestingPermission':
      return 'Waiting for Android VPN permission';
    case 'starting':
    case 'startingBlocking':
      return 'Creating a blocking Android route';
    case 'reconnecting':
      return 'Reconnecting the MASQ data path';
    case 'active':
      return disposition === 'masq'
        ? 'Route ready'
        : disposition === 'blocked'
          ? 'Route health mismatch · traffic blocked'
          : 'Route health mismatch · capture not confirmed';
    case 'stopping':
      return 'Stopping';
    case 'revoked':
      return 'Android VPN permission revoked';
    case 'blocked':
      return 'Waiting for a safe MASQ route';
    default:
      return 'Off';
  }
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
  routingStatus: {
    backgroundColor: '#102338',
    borderColor: '#31506C',
    borderRadius: radii.medium,
    borderWidth: 1,
    marginBottom: 14,
    padding: 14,
  },
  routingStatusTitle: {
    color: colors.white,
    fontSize: 15,
    fontWeight: '900',
    marginBottom: 5,
  },
  routingStatusRow: {
    borderTopColor: '#29445E',
    borderTopWidth: 1,
    flexDirection: 'row',
    justifyContent: 'space-between',
    paddingVertical: 9,
  },
  routingStatusLabel: {
    color: colors.muted,
    fontSize: 12,
    fontWeight: '700',
    marginRight: 12,
  },
  routingStatusValue: {
    color: colors.white,
    flex: 1,
    fontSize: 12,
    fontWeight: '700',
    textAlign: 'right',
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
