import React from 'react';
import { Alert, PermissionsAndroid, Platform, Text } from 'react-native';
import ReactTestRenderer from 'react-test-renderer';

import { TrafficRoutingScreen } from '../src/screens/TrafficRoutingScreen';
import type { SystemTunnelStatus } from '../src/core/systemTunnel';

const OFF_STATUS: SystemTunnelStatus = {
  active: false,
  appliedMode: 'off',
  appliedRevision: null,
  appliedSelectedApps: [],
  lastError: null,
  mode: 'off',
  phase: 'off',
  selectedApps: [],
  supported: true,
  trafficDisposition: 'off',
};

const ROUTABLE_APPS = [
  { id: 'org.example.app', label: 'Example' },
  { id: 'org.example.video', label: 'Video' },
];

function versionedStatus(
  overrides: Partial<SystemTunnelStatus> = {},
): SystemTunnelStatus {
  return {
    ...OFF_STATUS,
    alwaysOn: false,
    coreRouteReady: false,
    desiredMode: 'off',
    desiredRevision: null,
    desiredSelectedApps: [],
    failClosedDesired: false,
    lockdown: false,
    routingPhase: 'off',
    schemaVersion: 2,
    trafficObserved: false,
    translatorReady: false,
    tunPresent: false,
    ...overrides,
  };
}

function screen(
  status: SystemTunnelStatus,
  onApply: jest.Mock = jest.fn().mockResolvedValue(undefined),
) {
  return (
    <TrafficRoutingScreen
      busy={false}
      connected
      onApply={onApply}
      onBack={jest.fn()}
      routableApps={ROUTABLE_APPS}
      status={status}
    />
  );
}

function findControl(
  renderer: ReactTestRenderer.ReactTestRenderer,
  label: string,
) {
  return renderer.root.find(
    node =>
      ['button', 'checkbox', 'radio'].includes(node.props.accessibilityRole) &&
      node.findAllByType(Text).some(text => text.props.children === label),
  );
}

describe('TrafficRoutingScreen native support gate', () => {
  afterEach(() => {
    jest.restoreAllMocks();
  });

  it('hides system-routing controls when native support is false', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        screen({ ...OFF_STATUS, supported: false }),
      );
    });

    const rendered = JSON.stringify(renderer.toJSON());
    expect(rendered).toContain('System tunnel unavailable');
    expect(rendered).not.toContain('Whole device');
    expect(rendered).not.toContain('Selected apps');
    expect(rendered).not.toContain('Experimental community routing');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('shows native-supported controls with an explicit limitations disclosure', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(screen(OFF_STATUS));
    });

    const rendered = JSON.stringify(renderer.toJSON());
    expect(rendered).toContain('Whole device');
    expect(rendered).toContain('Selected apps');
    expect(rendered).toContain('Experimental community routing');
    expect(rendered).toContain(
      'Only IPv4 TCP connections to port 443 are sent through MASQ',
    );
    expect(rendered).toContain('DNS is handled virtually');
    expect(rendered).toContain('All other captured IP traffic');
    expect(rendered).toContain('ICMP, and unknown');
    expect(rendered).toContain('encrypted connection to example.com');
    expect(rendered).toContain('no page body is downloaded');
    expect(rendered).toContain(
      'MASQ packages installed when the route is created are excluded',
    );
    expect(rendered).toContain('safely rebuilds that UID scope');
    expect(rendered).toContain('status returns to MASQ');
    expect(rendered).toContain('loopback proxy is unauthenticated');
    expect(rendered).toContain('must allow notifications before activation');
    expect(rendered).toContain('traffic can return to the direct connection');
    expect(rendered).toContain('Always-on VPN');
    expect(rendered).toContain('Block connections without VPN');
    expect(rendered.toLowerCase()).not.toContain('all data');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('preserves an unsaved selected-app draft across unchanged status polls', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    const initialStatus = {
      ...OFF_STATUS,
      appliedRevision: 4,
    };
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(screen(initialStatus));
    });
    ReactTestRenderer.act(() => {
      findControl(renderer, 'Selected apps').props.onPress();
    });
    const privacyDisclosure = JSON.stringify(renderer.toJSON());
    expect(privacyDisclosure).toContain(
      'Package IDs and the consent timestamp',
    );
    expect(privacyDisclosure).toContain(
      'apps sharing a UID can share routing',
    );
    expect(privacyDisclosure).toContain(
      'work-profile copies are a separate user scope',
    );
    expect(privacyDisclosure).toContain('attached restricted profiles');
    expect(privacyDisclosure).toContain(
      'Package-to-UID rules are captured only',
    );
    ReactTestRenderer.act(() => {
      findControl(renderer, 'Example').props.onPress();
    });
    expect(
      findControl(renderer, 'Example').props.accessibilityState.checked,
    ).toBe(true);

    ReactTestRenderer.act(() => {
      renderer.update(
        screen({
          ...initialStatus,
          appliedSelectedApps: [],
          lastError: 'A harmless polling diagnostic.',
          selectedApps: [],
        }),
      );
    });

    expect(
      findControl(renderer, 'Selected apps').props.accessibilityState.checked,
    ).toBe(true);
    expect(
      findControl(renderer, 'Example').props.accessibilityState.checked,
    ).toBe(true);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('does not replace a dirty draft when an unrelated desired revision arrives', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(screen(versionedStatus()));
    });
    ReactTestRenderer.act(() => {
      findControl(renderer, 'Selected apps').props.onPress();
    });
    ReactTestRenderer.act(() => {
      findControl(renderer, 'Example').props.onPress();
    });

    ReactTestRenderer.act(() => {
      renderer.update(
        screen(
          versionedStatus({
            desiredRevision: 17,
          }),
        ),
      );
    });

    expect(
      findControl(renderer, 'Selected apps').props.accessibilityState.checked,
    ).toBe(true);
    expect(
      findControl(renderer, 'Example').props.accessibilityState.checked,
    ).toBe(true);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps the requested whole-device scope while capture is starting', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(screen(versionedStatus()));
    });
    ReactTestRenderer.act(() => {
      findControl(renderer, 'Whole device').props.onPress();
    });

    ReactTestRenderer.act(() => {
      renderer.update(
        screen(
          versionedStatus({
            desiredMode: 'wholeDevice',
            desiredRevision: 12,
            mode: 'wholeDevice',
            phase: 'starting',
            routingPhase: 'startingBlocking',
            trafficDisposition: 'directRisk',
          }),
        ),
      );
    });

    const rendered = JSON.stringify(renderer.toJSON());
    expect(rendered).toContain('Requested');
    expect(rendered).toContain('Whole device · compatible HTTPS only');
    expect(rendered).toContain('Captured');
    expect(rendered).toContain('Not captured');
    expect(rendered).toContain('Creating a blocking Android route');
    expect(rendered).not.toContain('Retry MASQ route');
    expect(rendered).not.toContain('Private browser only');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('offers a reachable retry that reuses the exact saved scope', async () => {
    const onApply = jest.fn().mockResolvedValue(undefined);
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        screen(
          {
            ...OFF_STATUS,
            appliedMode: 'selectedApps',
            appliedRevision: 9,
            appliedSelectedApps: ['org.example.video'],
            mode: 'selectedApps',
            phase: 'blocked',
            selectedApps: ['org.example.video'],
            trafficDisposition: 'blocked',
          },
          onApply,
        ),
      );
    });

    await ReactTestRenderer.act(async () => {
      await findControl(renderer, 'Retry MASQ route').props.onPress();
    });

    expect(onApply).toHaveBeenCalledWith('selectedApps', [
      'org.example.video',
    ]);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('distinguishes a ready route from observed external app traffic', () => {
    const active = versionedStatus({
      active: true,
      appliedMode: 'wholeDevice',
      appliedRevision: 14,
      coreRouteReady: true,
      desiredMode: 'wholeDevice',
      desiredRevision: 14,
      mode: 'wholeDevice',
      phase: 'active',
      routingPhase: 'active',
      trafficDisposition: 'masq',
      translatorReady: true,
      tunPresent: true,
    });
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(screen(active));
    });

    expect(JSON.stringify(renderer.toJSON())).toContain(
      'MASQ route ready · waiting for compatible app traffic',
    );
    ReactTestRenderer.act(() => {
      renderer.update(screen({ ...active, trafficObserved: true }));
    });
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'Captured HTTPS session reached the local MASQ adapter',
    );
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('refuses Android 13 dogfood activation when notification permission is denied', async () => {
    const originalOs = Platform.OS;
    const originalVersion = Platform.Version;
    Object.defineProperty(Platform, 'OS', {
      configurable: true,
      value: 'android',
    });
    Object.defineProperty(Platform, 'Version', {
      configurable: true,
      value: 33,
    });
    jest.spyOn(PermissionsAndroid, 'check').mockResolvedValue(false);
    jest
      .spyOn(PermissionsAndroid, 'request')
      .mockResolvedValue(PermissionsAndroid.RESULTS.DENIED);
    const onApply = jest.fn().mockResolvedValue(undefined);
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    try {
      ReactTestRenderer.act(() => {
        renderer = ReactTestRenderer.create(
          screen(
            {
              ...OFF_STATUS,
              appliedMode: 'wholeDevice',
              appliedRevision: 10,
              mode: 'wholeDevice',
              phase: 'blocked',
              trafficDisposition: 'blocked',
            },
            onApply,
          ),
        );
      });

      await ReactTestRenderer.act(async () => {
        await findControl(renderer, 'Retry MASQ route').props.onPress();
      });

      expect(onApply).not.toHaveBeenCalled();
      expect(JSON.stringify(renderer.toJSON())).toContain(
        'Allow notifications before starting community system routing',
      );
    } finally {
      ReactTestRenderer.act(() => renderer?.unmount());
      Object.defineProperty(Platform, 'OS', {
        configurable: true,
        value: originalOs,
      });
      Object.defineProperty(Platform, 'Version', {
        configurable: true,
        value: originalVersion,
      });
    }
  });

  it('allows Android 13 activation when the ongoing notice is permitted', async () => {
    const originalOs = Platform.OS;
    const originalVersion = Platform.Version;
    Object.defineProperty(Platform, 'OS', {
      configurable: true,
      value: 'android',
    });
    Object.defineProperty(Platform, 'Version', {
      configurable: true,
      value: 33,
    });
    jest.spyOn(PermissionsAndroid, 'check').mockResolvedValue(true);
    jest
      .spyOn(PermissionsAndroid, 'request')
      .mockResolvedValue(PermissionsAndroid.RESULTS.GRANTED);
    const onApply = jest.fn().mockResolvedValue(undefined);
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    try {
      ReactTestRenderer.act(() => {
        renderer = ReactTestRenderer.create(
          screen(
            {
              ...OFF_STATUS,
              appliedMode: 'wholeDevice',
              appliedRevision: 11,
              mode: 'wholeDevice',
              phase: 'blocked',
              trafficDisposition: 'blocked',
            },
            onApply,
          ),
        );
      });

      await ReactTestRenderer.act(async () => {
        await findControl(renderer, 'Retry MASQ route').props.onPress();
      });

      expect(onApply).toHaveBeenCalledWith('wholeDevice', []);
      expect(PermissionsAndroid.request).not.toHaveBeenCalled();
    } finally {
      ReactTestRenderer.act(() => renderer?.unmount());
      Object.defineProperty(Platform, 'OS', {
        configurable: true,
        value: originalOs,
      });
      Object.defineProperty(Platform, 'Version', {
        configurable: true,
        value: originalVersion,
      });
    }
  });

  it('shows a new desired native revision separately from applied capture', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        screen({ ...OFF_STATUS, appliedRevision: 4 }),
      );
    });
    ReactTestRenderer.act(() => {
      findControl(renderer, 'Selected apps').props.onPress();
    });
    ReactTestRenderer.act(() => {
      findControl(renderer, 'Example').props.onPress();
    });

    ReactTestRenderer.act(() => {
      renderer.update(
        screen({
          ...OFF_STATUS,
          appliedMode: 'wholeDevice',
          appliedRevision: 5,
          mode: 'wholeDevice',
        }),
      );
    });

    const rendered = JSON.stringify(renderer.toJSON());
    expect(rendered).toContain('Requested');
    expect(rendered).toContain('Whole device · compatible HTTPS only');
    expect(rendered).toContain('Captured');
    expect(rendered).not.toContain('Private browser only');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('requires explicit confirmation before applying a selected-app draft', async () => {
    const onApply = jest.fn().mockResolvedValue(undefined);
    const alert = jest.spyOn(Alert, 'alert').mockImplementation(jest.fn());
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(screen(OFF_STATUS, onApply));
    });
    ReactTestRenderer.act(() => {
      findControl(renderer, 'Selected apps').props.onPress();
    });
    ReactTestRenderer.act(() => {
      findControl(renderer, 'Example').props.onPress();
    });
    ReactTestRenderer.act(() => {
      findControl(renderer, 'Apply selected-app routing').props.onPress();
    });

    expect(onApply).not.toHaveBeenCalled();
    expect(alert).toHaveBeenCalledWith(
      'Confirm unsafe system routing',
      expect.stringContaining('not a VPN safety guarantee'),
      expect.any(Array),
    );
    const buttons = alert.mock.calls[0][2]!;
    const confirm = buttons.find(
      button => button.text === 'Apply community route',
    );
    await ReactTestRenderer.act(async () => {
      confirm?.onPress?.();
      await Promise.resolve();
    });
    expect(onApply).toHaveBeenCalledWith('selectedApps', ['org.example.app']);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('requires confirmation before turning off an active system route', async () => {
    const onApply = jest.fn().mockResolvedValue(undefined);
    const alert = jest.spyOn(Alert, 'alert').mockImplementation(jest.fn());
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        screen(
          {
            ...OFF_STATUS,
            active: true,
            appliedMode: 'wholeDevice',
            appliedRevision: 6,
            mode: 'wholeDevice',
            phase: 'active',
            trafficDisposition: 'masq',
          },
          onApply,
        ),
      );
    });

    ReactTestRenderer.act(() => {
      findControl(renderer, 'Turn off system routing').props.onPress();
    });
    expect(onApply).not.toHaveBeenCalled();
    const buttons = alert.mock.calls[0][2]!;
    const confirm = buttons.find(button => button.text === 'Turn off');
    await ReactTestRenderer.act(async () => {
      confirm?.onPress?.();
      await Promise.resolve();
    });
    expect(onApply).toHaveBeenCalledWith('off', []);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('renders direct-risk as an actionable warning, never as active protection', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        screen({
          ...OFF_STATUS,
          appliedMode: 'off',
          appliedRevision: null,
          lastError: 'PROCESS_RESTARTED',
          mode: 'wholeDevice',
          phase: 'blocked',
          trafficDisposition: 'directRisk',
        }),
      );
    });

    const rendered = JSON.stringify(renderer.toJSON());
    expect(rendered).toContain('Traffic may be direct');
    expect(rendered).toContain('Do not assume MASQ routing');
    expect(rendered).toContain('Turn off system routing');
    expect(rendered).not.toContain('dogfood route active');
    expect(rendered.toLowerCase()).not.toContain('protected');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('renders captured blocking distinctly from direct-risk', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        screen({
          ...OFF_STATUS,
          lastError: 'CORE_ROUTE_UNAVAILABLE',
          mode: 'wholeDevice',
          phase: 'blocked',
          trafficDisposition: 'blocked',
        }),
      );
    });

    const rendered = JSON.stringify(renderer.toJSON());
    expect(rendered).toContain('Captured traffic is blocked');
    expect(rendered).toContain(
      'instead of sent through the normal connection',
    );
    expect(rendered).not.toContain('Traffic may be direct');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('does not call an inconsistent native active phase ready', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        screen(
          versionedStatus({
            active: true,
            appliedMode: 'wholeDevice',
            appliedRevision: 22,
            coreRouteReady: false,
            desiredMode: 'wholeDevice',
            desiredRevision: 22,
            mode: 'wholeDevice',
            phase: 'active',
            routingPhase: 'active',
            trafficDisposition: 'masq',
            translatorReady: true,
            tunPresent: true,
          }),
        ),
      );
    });

    const rendered = JSON.stringify(renderer.toJSON());
    expect(rendered).toContain('Route health mismatch · traffic blocked');
    expect(rendered).not.toContain('Route ready');
    ReactTestRenderer.act(() => renderer.unmount());
  });
});
