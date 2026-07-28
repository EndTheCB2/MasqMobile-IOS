import React from 'react';
import { Text } from 'react-native';
import ReactTestRenderer from 'react-test-renderer';

import { TrafficRoutingScreen } from '../src/screens/TrafficRoutingScreen';
import type { SystemTunnelStatus } from '../src/core/systemTunnel';

const OFF_STATUS: SystemTunnelStatus = {
  active: false,
  lastError: null,
  mode: 'off',
  phase: 'off',
  selectedApps: [],
  supported: true,
};

describe('TrafficRoutingScreen public release gate', () => {
  it('does not offer a new whole-device or selected-app tunnel', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        <TrafficRoutingScreen
          busy={false}
          connected
          onApply={jest.fn()}
          onBack={jest.fn()}
          routableApps={[{ id: 'org.example.app', label: 'Example' }]}
          status={OFF_STATUS}
        />,
      );
    });

    const screen = JSON.stringify(renderer.toJSON());
    expect(screen).toContain('Experimental system routing paused');
    expect(screen).toContain('Use the isolated MASQ Private browser for now');
    expect(screen).not.toContain('Whole device');
    expect(screen).not.toContain('Selected apps');
    expect(screen).not.toContain('Always-on VPN');
    expect(screen).not.toContain('fail-closed');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('can turn off a tunnel left active by an earlier preview', async () => {
    const onApply = jest.fn().mockResolvedValue(undefined);
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        <TrafficRoutingScreen
          busy={false}
          connected
          onApply={onApply}
          onBack={jest.fn()}
          routableApps={[]}
          status={{
            ...OFF_STATUS,
            active: true,
            mode: 'wholeDevice',
            phase: 'active',
          }}
        />,
      );
    });

    const turnOff = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node
          .findAllByType(Text)
          .some(text => text.props.children === 'Turn off system routing'),
    );
    await ReactTestRenderer.act(async () => {
      await turnOff.props.onPress();
    });
    expect(onApply).toHaveBeenCalledWith('off', []);
    ReactTestRenderer.act(() => renderer.unmount());
  });
});
