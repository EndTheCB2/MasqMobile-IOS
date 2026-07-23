import { useEffect, useState } from 'react';
import {
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import type {
  RoutableApp,
  SystemTunnelMode,
  SystemTunnelStatus,
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
  const [choice, setChoice] = useState<SystemTunnelMode>(status.mode);
  const [selectedApps, setSelectedApps] = useState(status.selectedApps);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setChoice(status.mode);
    setSelectedApps(status.selectedApps);
  }, [status.mode, status.selectedApps]);

  const apply = async (mode: SystemTunnelMode, apps: string[]) => {
    setError(null);
    try {
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

  const unavailableReason =
    Platform.OS === 'ios'
      ? 'This iOS build has no signed Packet Tunnel entitlement. The private browser remains available and fail-closed.'
      : 'The native packet-tunnel library is unavailable in this build.';

  return (
    <View style={styles.screen}>
      <ScreenHeader title="Traffic routing" onBack={onBack} />
      <ScrollView
        contentContainerStyle={styles.content}
        showsVerticalScrollIndicator={false}
      >
        <Text style={styles.eyebrow}>PROTECTION SCOPE</Text>
        <Text style={styles.title}>Choose what uses MASQ</Text>
        <Text style={styles.intro}>
          System routing is fail-closed: if the MASQ route or packet translator
          stops, captured apps stay blocked instead of falling back to your
          direct connection.
        </Text>
        <ErrorBanner message={error || status.lastError} />

        {!status.supported ? (
          <View style={styles.warning}>
            <Text style={styles.warningTitle}>System tunnel unavailable</Text>
            <Text style={styles.warningText}>{unavailableReason}</Text>
          </View>
        ) : null}

        <ScopeCard
          active={choice === 'off'}
          detail="Only the isolated MASQ browser is proxied. Other apps use their normal connection."
          label="Private browser only"
          onPress={() => {
            setChoice('off');
            if (status.phase !== 'off') apply('off', []).catch(() => undefined);
          }}
        />
        <ScopeCard
          active={choice === 'wholeDevice'}
          detail="Android captures every other app. MASQ Mobile's own management traffic is excluded to prevent a VPN loop."
          disabled={!status.supported || status.phase !== 'off'}
          label="Whole device"
          onPress={() => setChoice('wholeDevice')}
        />
        <ScopeCard
          active={choice === 'selectedApps'}
          detail="Capture only the Android apps you select below. App identities remain on this device."
          disabled={!status.supported || status.phase !== 'off'}
          label="Selected apps"
          onPress={() => setChoice('selectedApps')}
        />

        {choice === 'selectedApps' &&
        status.phase === 'off' &&
        status.supported ? (
          <View style={styles.appsCard}>
            <Text style={styles.appsTitle}>Apps routed through MASQ</Text>
            <Text style={styles.appsHelper}>
              Only launchable apps are listed. Choose at least one.
            </Text>
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

        {status.phase === 'off' && choice !== 'off' ? (
          <Button
            busy={busy}
            disabled={
              busy ||
              !connected ||
              !status.supported ||
              (choice === 'selectedApps' && selectedApps.length === 0)
            }
            label={
              connected
                ? choice === 'wholeDevice'
                  ? 'Protect whole device'
                  : 'Protect selected apps'
                : 'Connect MASQ first'
            }
            onPress={() =>
              apply(choice, choice === 'selectedApps' ? selectedApps : [])
            }
          />
        ) : status.phase !== 'off' ? (
          <Button
            busy={busy}
            label={
              status.phase === 'starting'
                ? 'Starting system tunnel…'
                : 'Turn off system routing'
            }
            onPress={() => apply('off', [])}
            tone="danger"
          />
        ) : null}

        <Text style={styles.footnote}>
          For the strongest Android kill switch, enable “Always-on VPN” and
          “Block connections without VPN” for MASQ in system VPN settings. UDP
          other than DNS is intentionally blocked because the current MASQ
          consumer proxy carries TCP/HTTP CONNECT traffic.
        </Text>
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
  footnote: {
    color: '#5E7487',
    fontSize: 11,
    lineHeight: 17,
    marginTop: 15,
    textAlign: 'center',
  },
});
