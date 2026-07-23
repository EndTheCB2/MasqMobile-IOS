import React from 'react';
import { Text } from 'react-native';
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
    expect(content).toContain('MASQ hops and exit-country settings do not apply');
    expect(content).toContain('Wallet secret stays on this device');

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
});
