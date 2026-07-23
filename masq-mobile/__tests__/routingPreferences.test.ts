import { exitCountryOptionsForAvailability } from '../src/core/routingPreferences';

describe('live MASQ exit-country options', () => {
  it('keeps the full picker until the neighborhood inventory is ready', () => {
    const options = exitCountryOptionsForAvailability([], null, false);

    expect(options.length).toBeGreaterThan(10);
  });

  it('shows only live countries plus automatic routing', () => {
    const options = exitCountryOptionsForAvailability(
      ['US', 'BE', 'invalid'],
      null,
      true,
    );

    expect(options.map(option => option.code)).toEqual([null, 'BE', 'US']);
  });

  it('retains a selected country when it just became unavailable', () => {
    const options = exitCountryOptionsForAvailability(['US'], 'BE', true);

    expect(options.map(option => option.code)).toEqual([null, 'BE', 'US']);
  });
});
