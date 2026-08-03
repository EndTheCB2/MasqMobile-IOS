import React from 'react';
import { Platform, Text } from 'react-native';
import ReactTestRenderer from 'react-test-renderer';

import { PrivacyScreen } from '../src/screens/PrivacyScreen';

describe('PrivacyScreen', () => {
  it('explains external processing and exposes explicit legal links', () => {
    const onOpenPrivacyPolicy = jest.fn();
    const onOpenSource = jest.fn();
    const onOpenSupport = jest.fn();
    let renderer!: ReactTestRenderer.ReactTestRenderer;

    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        <PrivacyScreen
          onBack={jest.fn()}
          onOpenPrivacyPolicy={onOpenPrivacyPolicy}
          onOpenSource={onOpenSource}
          onOpenSupport={onOpenSupport}
          systemRoutingSupported={false}
        />,
      );
    });

    const content = JSON.stringify(renderer.toJSON());
    expect(content).toContain('MASQ Private is fail-closed');
    expect(content).toContain('never switches to Direct');
    expect(content).toContain('Direct browsing is a separate choice');
    expect(content).toContain('public IP of your current connection or VPN');
    expect(content).toContain(
      'stops any active MASQ connection and system routing',
    );
    expect(content).toContain(
      'MASQ hops and exit-country settings do not apply',
    );
    expect(content).toContain('Wallet secret stays on this device');
    expect(content).toContain(
      'Debt settlement is an explicit blockchain action',
    );
    expect(content).toContain('tapping Settle now is the final confirmation');
    expect(content).toContain('public on Base');
    expect(content).toContain('never retried automatically');
    expect(content).toContain('Browser sessions are temporary by default');
    expect(content).toContain('Remember sign-in');
    expect(content).toContain('Temporary app switching keeps the page ready');
    expect(content).toContain('approve a YouTube sign-in');
    expect(content).toContain("behind MASQ Mobile's privacy shield");
    expect(content).toContain('may continue network activity while hidden');
    expect(content).toContain('without fallback');
    expect(content).toContain(
      'Explicitly closing the browser ends the routing lease',
    );
    expect(content).toContain('removes the app process under memory pressure');
    expect(content).toContain('ENS preview uses an HTTPS gateway');
    expect(content).toContain('never falls back to search');
    expect(content).toContain('Cookie protection is local and optional');
    expect(content).toContain('never selects Accept');
    expect(content).toContain('Choose Timpi or DuckDuckGo for searches');
    expect(content).toContain('receives the search query');
    expect(content).toContain('MASQ exit IP in MASQ Private');
    expect(content).toContain('stores only your provider choice');
    expect(content).toContain(
      'does not store or synchronize search queries or search history',
    );

    const privacyButton = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node
          .findAllByType(Text)
          .some(text => text.props.children === 'Read full privacy policy'),
    );
    const sourceButton = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node
          .findAllByType(Text)
          .some(text => text.props.children === 'View source and licences'),
    );
    const supportButton = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node
          .findAllByType(Text)
          .some(text => text.props.children === 'Open support'),
    );

    ReactTestRenderer.act(() => privacyButton.props.onPress());
    ReactTestRenderer.act(() => sourceButton.props.onPress());
    ReactTestRenderer.act(() => supportButton.props.onPress());
    expect(onOpenPrivacyPolicy).toHaveBeenCalledTimes(1);
    expect(onOpenSource).toHaveBeenCalledTimes(1);
    expect(onOpenSupport).toHaveBeenCalledTimes(1);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('shows complete system-routing data flow only in supported Android dogfood', () => {
    const originalPlatform = Platform.OS;
    Object.defineProperty(Platform, 'OS', {
      configurable: true,
      value: 'android',
    });
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    try {
      ReactTestRenderer.act(() => {
        renderer = ReactTestRenderer.create(
          <PrivacyScreen
            onBack={jest.fn()}
            onOpenPrivacyPolicy={jest.fn()}
            onOpenSource={jest.fn()}
            onOpenSupport={jest.fn()}
            systemRoutingSupported
          />,
        );
      });
      const content = JSON.stringify(renderer.toJSON());
      expect(content).toContain('Android community system routing is limited');
      expect(content).toContain('IPv4 TCP/443 and virtual DNS');
      expect(content).toContain('All other captured IP traffic');
      expect(content).toContain('ICMP and unknown transports');
      expect(content).toContain('encrypted HEAD request to example.com');
      expect(content).toContain('no page body is downloaded');
      expect(content).toContain(
        'Selected package IDs and the consent timestamp',
      );
      expect(content).toContain('shared-UID apps can share routing');
      expect(content).toContain('attached restricted profiles');
      expect(content).toContain('work profiles are separate');
      expect(content).toContain('Turn routing off before installing');
      expect(content).toContain('must grant notification permission');
      expect(content).toContain('turning routing off never requires');
      expect(content).toContain('service/app-process death can restore direct');
      expect(content).toContain('loopback MASQ proxy is unauthenticated');
      expect(content).toContain('must not be distributed publicly');
    } finally {
      ReactTestRenderer.act(() => renderer?.unmount());
      Object.defineProperty(Platform, 'OS', {
        configurable: true,
        value: originalPlatform,
      });
    }
  });
});
