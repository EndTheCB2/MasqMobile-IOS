import {
  discoverEntryNodes,
  normalizeNodeFinderBaseUrl,
} from '../src/core/discovery';

const nodeFinderUrl = 'https://nodes.example.org';

const nodeA =
  'masq://base-mainnet:68ce7epLjmPtnQi-Gy1vqJdvt3kAdYkJTyjR9EmfvFQ@45.76.232.183:44845';
const nodeB =
  'masq://base-mainnet:GBTuCfVAzt1uU9PN2VU4ibJtw2MlZfKBXoK9pgG9-Eo@45.32.40.127:53602';

function response(body: string, ok = true): Response {
  return {
    ok,
    status: ok ? 200 : 503,
    text: async () => body,
  } as Response;
}

describe('MASQ entry node discovery', () => {
  it('fetches two unique nodes from the official node-finder', async () => {
    const fetchImpl = jest
      .fn<ReturnType<typeof fetch>, Parameters<typeof fetch>>()
      .mockResolvedValueOnce(response(nodeA))
      .mockResolvedValueOnce(response(nodeB));

    await expect(
      discoverEntryNodes('base-mainnet', {baseUrl: nodeFinderUrl, fetchImpl}),
    ).resolves.toEqual([nodeA, nodeB]);
    expect(fetchImpl).toHaveBeenCalledTimes(2);
    expect(fetchImpl.mock.calls[0][0]).toBe(
      `${nodeFinderUrl}/randomnode/base-mainnet/masqpublic1`,
    );
  });

  it('retries duplicate and malformed responses', async () => {
    const fetchImpl = jest
      .fn<ReturnType<typeof fetch>, Parameters<typeof fetch>>()
      .mockResolvedValueOnce(response(nodeA))
      .mockResolvedValueOnce(response(nodeA))
      .mockResolvedValueOnce(response('not-a-descriptor'))
      .mockResolvedValueOnce(response(JSON.stringify(nodeB)));

    await expect(
      discoverEntryNodes('base-mainnet', {baseUrl: nodeFinderUrl, fetchImpl}),
    ).resolves.toEqual([nodeA, nodeB]);
    expect(fetchImpl).toHaveBeenCalledTimes(4);
  });

  it('fails closed when two valid nodes cannot be found', async () => {
    const fetchImpl = jest
      .fn<ReturnType<typeof fetch>, Parameters<typeof fetch>>()
      .mockResolvedValue(response('masq://base-sepolia:key@example.org:1234'));

    await expect(
      discoverEntryNodes('base-mainnet', {
        baseUrl: nodeFinderUrl,
        fetchImpl,
        maxAttempts: 2,
      }),
    ).rejects.toThrow(/could not find 2 unique entry nodes/i);
  });

  it('uses the release-provided HTTPS node-finder and rejects unsafe URLs', async () => {
    const fetchImpl = jest
      .fn<ReturnType<typeof fetch>, Parameters<typeof fetch>>()
      .mockResolvedValueOnce(response(nodeA))
      .mockResolvedValueOnce(response(nodeB));

    await discoverEntryNodes('base-mainnet', {
      baseUrl: 'https://nodes.example.org/',
      fetchImpl,
    });
    expect(fetchImpl.mock.calls[0][0]).toBe(
      'https://nodes.example.org/randomnode/base-mainnet/masqpublic1',
    );
    expect(() => normalizeNodeFinderBaseUrl('http://nodes.example.org')).toThrow(
      /verified HTTPS MASQ node-finder/i,
    );
    expect(() =>
      normalizeNodeFinderBaseUrl('https://token@nodes.example.org'),
    ).toThrow(/verified HTTPS MASQ node-finder/i);
  });
});
