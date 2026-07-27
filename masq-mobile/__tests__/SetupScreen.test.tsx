import React from 'react';
import { TextInput } from 'react-native';
import ReactTestRenderer from 'react-test-renderer';

import { SetupScreen } from '../src/screens/SetupScreen';
import { DEFAULT_SETUP } from '../src/core/types';

const neighbors = [
  'masq://base-mainnet:68ce7epLjmPtnQi-Gy1vqJdvt3kAdYkJTyjR9EmfvFQ@45.76.232.183:44845',
  'masq://base-mainnet:GBTuCfVAzt1uU9PN2VU4ibJtw2MlZfKBXoK9pgG9-Eo@45.32.40.127:53602',
];

describe('SetupScreen wallet privacy', () => {
  it('masks recovery words by default and requires an explicit reveal', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        <SetupScreen
          availableExitCountries={[]}
          busy={false}
          error={null}
          exitCountryInventoryReady={false}
          hasWallet={false}
          initial={{
            ...DEFAULT_SETUP,
            neighbors,
            walletSecret: 'synthetic recovery words',
          }}
          onBack={jest.fn()}
          onSave={jest.fn()}
        />,
      );
    });

    const recoveryInput = () =>
      renderer.root.find(
        node =>
          node.type === TextInput &&
          node.props.accessibilityLabel === 'Recovery phrase input',
      );

    expect(recoveryInput().props.secureTextEntry).toBe(true);
    expect(recoveryInput().props.autoComplete).toBe('off');
    expect(recoveryInput().props.importantForAutofill).toBe(
      'noExcludeDescendants',
    );
    expect(recoveryInput().props.textContentType).toBe('none');

    const showButton = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node.props.accessibilityLabel === 'Show recovery phrase',
    );
    ReactTestRenderer.act(() => showButton.props.onPress());
    expect(recoveryInput().props.secureTextEntry).toBe(false);

    const hideButton = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node.props.accessibilityLabel === 'Hide recovery phrase',
    );
    ReactTestRenderer.act(() => hideButton.props.onPress());
    expect(recoveryInput().props.secureTextEntry).toBe(true);

    ReactTestRenderer.act(() => renderer.unmount());
  });
});
