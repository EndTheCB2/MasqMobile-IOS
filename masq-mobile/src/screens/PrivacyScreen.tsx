import { Platform, ScrollView, StyleSheet, Text, View } from 'react-native';

import { Button, Card, ScreenHeader } from '../ui/components';
import { colors } from '../ui/theme';

interface Props {
  onBack: () => void;
  onOpenPrivacyPolicy: () => void;
  onOpenSource: () => void;
  onOpenSupport: () => void;
  systemRoutingSupported: boolean;
}

export function PrivacyScreen({
  onBack,
  onOpenPrivacyPolicy,
  onOpenSource,
  onOpenSupport,
  systemRoutingSupported,
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
          title="Debt settlement is an explicit blockchain action"
          text="Review MASQ debts shows the amount, creditor count and an estimated Base L2 fee before anything is signed. No device code or biometric check is used: tapping Settle now is the final confirmation. The wallet signs locally, but submitted transaction hashes, wallet addresses, token transfers and fees are public on Base. The displayed estimate excludes Base's variable L1 data fee, so the final fee can be higher. An uncertain submission is never retried automatically."
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
          title="Browser sessions are temporary by default"
          text={
            Platform.OS === 'ios'
              ? "MASQ Private and Direct use separate temporary website stores. If you explicitly enable Remember sign-in for a site, WebKit retains that session in the selected MASQ or Direct profile. Sign-in providers and other domains reached by redirects may also retain data in that profile. MASQ never extracts or stores the password. Forget this site removes that site's origin data; Clear all remembered sign-ins and Reset everything clear the retained profiles."
              : "Cookies and website storage are temporary by default. Remember sign-in is offered only when this Android WebView can isolate browser profiles. Sign-in providers and other domains reached by redirects may also retain data in that site's profile. MASQ never extracts or stores the password. Forget and reset controls clear retained profiles."
          }
        />
        <Disclosure
          title="Temporary app switching keeps the page ready"
          text="While a browser session is open, briefly switching to another app—for example to approve a YouTube sign-in—keeps the same page alive behind MASQ Mobile's privacy shield. The page retains its selected MASQ Private or Direct route without fallback and may continue network activity while hidden so the confirmation can complete. Explicitly closing the browser ends the routing lease. If iOS or Android removes the app process under memory pressure, the exact page or unfinished form may still be lost."
        />
        <Disclosure
          title="ENS preview uses an HTTPS gateway"
          text="Normalized ASCII or punycode .eth addresses are translated locally to the matching eth.limo HTTPS gateway address. The original .eth name stays visible in the address bar. eth.limo and its infrastructure can process the requested name, path and normal connection metadata. In MASQ Private it sees the exit IP; in Direct it sees the current public IP. A gateway failure never falls back to search, ordinary DNS or Direct browsing."
        />
        <Disclosure
          title="Cookie protection is local and optional"
          text="Balanced and Strict protection use versioned rules stored with the app. Supported consent managers are automated only when Reject optional cookies is enabled, and the app never selects Accept. Turning protection off for a site can restore compatibility but allows that site's cookies, ads and trackers under its own policy. MASQ records no browsing telemetry."
        />
        <Disclosure
          title="Choose Timpi or DuckDuckGo for searches"
          text="Text that is not recognized as a public web address is sent to your selected public search provider: Timpi or DuckDuckGo. That provider receives the search query and sees the apparent IP address: the MASQ exit IP in MASQ Private, or the public IP of your current connection or VPN in Direct. The provider may process the request under its own privacy policy. MASQ Mobile stores only your provider choice; it does not store or synchronize search queries or search history."
        />
        {Platform.OS === 'android' && systemRoutingSupported ? (
          <Disclosure
            title="Android community system routing is limited"
            text="Only the separately compiled MASQ community preview build can capture whole-device or selected-app traffic. It translates captured IPv4 TCP/443 and virtual DNS through MASQ. All other captured IP traffic—including other TCP ports, non-DNS UDP, IPv6, ICMP and unknown transports—is blocked only while capture remains valid. Activation completes a TLS handshake and an encrypted HEAD request to example.com through the MASQ exit; no page body is downloaded. Selected package IDs and the consent timestamp stay on this device. Android snapshots package-to-UID scope when the route is created: shared-UID apps can share routing, attached restricted profiles may receive the scope, and work profiles are separate. When a scoped app is installed, removed, enabled, disabled or updated, this build pauses translation and automatically rebuilds the UID scope. Use the affected app only after routing returns to MASQ; turn routing off if Android cannot confirm the rebuilt scope. Android 13 or later must grant notification permission before activation so the ongoing experimental-routing state stays visible; turning routing off never requires that permission. VPN revocation or service/app-process death can restore direct traffic; Always-on/lockdown is unsupported. The temporary loopback MASQ proxy is unauthenticated, so a malicious local app that discovers its port could consume the route and wallet funds. This private internal build must not be distributed publicly while local proxy authentication and audited process-death fail-closed behavior are unavailable."
          />
        ) : null}
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
          Effective 27 July 2026 · Review the online policy for the current
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
