import { Platform, ScrollView, StyleSheet, Text, View } from 'react-native';

import { Button, Card, ScreenHeader } from '../ui/components';
import { colors } from '../ui/theme';

interface Props {
  onBack: () => void;
  onOpenPrivacyPolicy: () => void;
  onOpenSource: () => void;
  onOpenSupport: () => void;
}

export function PrivacyScreen({
  onBack,
  onOpenPrivacyPolicy,
  onOpenSource,
  onOpenSupport,
}: Props) {
  return (
    <View style={styles.screen}>
      <ScreenHeader title="Privacy & legal" onBack={onBack} />
      <ScrollView
        contentContainerStyle={styles.content}
        showsVerticalScrollIndicator={false}
      >
        <Text style={styles.eyebrow}>TRANSPARENT BY DESIGN</Text>
        <Text style={styles.title}>Know where your data goes</Text>
        <Text style={styles.intro}>
          This independent, consume-only client has no developer-operated
          account, advertising, analytics or cross-app tracking.
        </Text>

        <Disclosure
          title="Wallet secret stays on this device"
          text="Your recovery phrase or private key is stored in device-bound secure storage. It is removed only when you choose Remove wallet or Reset everything. Never send recovery words to support."
        />
        <Disclosure
          title="MASQ Private is fail-closed"
          text="The selected blockchain RPC receives your IP address, wallet address and JSON-RPC requests. MASQ entry, relay and exit nodes process the routing metadata needed to carry traffic. Destination sites see the exit node's IP address. If a MASQ route is unavailable, MASQ Private blocks browsing and never switches to Direct."
        />
        <Disclosure
          title="Direct browsing is a separate choice"
          text="Browse without MASQ stops any active MASQ connection and system routing, then uses your normal internet connection. Websites see the public IP of your current connection or VPN, while your internet provider and DNS service can see normal connection metadata. MASQ hops and exit-country settings do not apply. The app enters this mode only after you confirm its warning."
        />
        <Disclosure
          title="Browser sessions are temporary"
          text={
            Platform.OS === 'ios'
              ? "MASQ Private and Direct use separate, isolated non-persistent website stores. Website cookies, cache and page history are not written to persistent website storage, but may remain in memory until the app's web-content process exits. Closing the browser or backgrounding the app blocks browser routing."
              : 'Cookies and website storage are cleared when either temporary browser starts and closes. Android WebView may still use app storage while a session is active. Closing the browser or backgrounding the app blocks browser routing.'
          }
        />
        <Disclosure
          title="Free-text searches use Timpi"
          text="Text that is not recognized as a public web address is sent to the public Timpi Search website. In MASQ Private, Timpi sees the exit node's IP address; in Direct, it sees the public IP of your current connection or VPN. Timpi may process search queries, approximate location and service logs under its own privacy policy. The app does not separately log or synchronize your searches."
        />
        <Disclosure
          title="Independent open-source build"
          text="This is not an official MASQ Project release. The source is provided under GPL-3.0-only and third-party components retain their own licences. MASQ names and marks belong to their respective owner."
        />

        <View style={styles.actions}>
          <Button
            label="Read full privacy policy"
            onPress={onOpenPrivacyPolicy}
          />
          <Button
            label="View source and licences"
            onPress={onOpenSource}
            tone="secondary"
          />
          <Button
            label="Open support"
            onPress={onOpenSupport}
            tone="secondary"
          />
        </View>
        <Text style={styles.footer}>
          Effective 23 July 2026 · Review the online policy for the current
          maintainer contact and any later changes.
        </Text>
      </ScrollView>
    </View>
  );
}

function Disclosure({ title, text }: { title: string; text: string }) {
  return (
    <View style={styles.card}>
      <Card>
        <Text style={styles.cardTitle}>{title}</Text>
        <Text style={styles.cardText}>{text}</Text>
      </Card>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { backgroundColor: colors.ink, flex: 1 },
  content: { paddingBottom: 44, paddingHorizontal: 20, paddingTop: 16 },
  eyebrow: {
    color: colors.violet,
    fontSize: 11,
    fontWeight: '900',
    letterSpacing: 1.7,
    marginBottom: 10,
  },
  title: {
    color: colors.white,
    fontSize: 30,
    fontWeight: '800',
    letterSpacing: -0.8,
    lineHeight: 36,
  },
  intro: {
    color: colors.muted,
    fontSize: 14,
    lineHeight: 21,
    marginBottom: 22,
    marginTop: 10,
  },
  card: { marginBottom: 12 },
  cardTitle: { color: colors.white, fontSize: 16, fontWeight: '800' },
  cardText: {
    color: colors.muted,
    fontSize: 13,
    lineHeight: 20,
    marginTop: 8,
  },
  actions: { gap: 10, marginTop: 8 },
  footer: {
    color: '#71879B',
    fontSize: 11,
    lineHeight: 17,
    marginTop: 18,
    textAlign: 'center',
  },
});
