import { afterEach, describe, expect, it, vi } from 'vitest';
import { fetchVersionTags } from './githubTags';

afterEach(() => vi.unstubAllGlobals());

describe('GitHub tags transport', () => {
  it('reads subsequent pages and sends no credentials', async () => {
    const first = Array.from({ length: 100 }, (_, index) => ({ name: `v1.0.${index}` }));
    const fetchMock = vi.fn().mockResolvedValueOnce(Response.json(first)).mockResolvedValueOnce(Response.json([{ name: 'v2.0.0' }]));
    vi.stubGlobal('fetch', fetchMock);
    const signal = new AbortController().signal;
    expect(await fetchVersionTags(signal)).toHaveLength(101);
    expect(fetchMock).toHaveBeenNthCalledWith(2, 'https://api.github.com/repos/LovelyTeams/LovelyCarve/tags?per_page=100&page=2', expect.objectContaining({ credentials: 'omit', redirect: 'error', signal }));
  });

  it('rejects malformed responses and HTTP failures', async () => {
    for (const response of [Response.json({ message: 'error' }), Response.json([{}]), new Response('', { status: 500 })]) {
      vi.stubGlobal('fetch', vi.fn().mockResolvedValue(response));
      await expect(fetchVersionTags(new AbortController().signal)).rejects.toMatchObject({ reason: 'response' });
    }
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('', { status: 429 })));
    await expect(fetchVersionTags(new AbortController().signal)).rejects.toMatchObject({ reason: 'rate-limit' });
  });

  it('fails explicitly when the bounded pagination cannot establish a complete list', async () => {
    const fetchMock = vi.fn().mockImplementation(async () => Response.json(Array.from({ length: 100 }, () => ({ name: 'v1.0.0' }))));
    vi.stubGlobal('fetch', fetchMock);
    await expect(fetchVersionTags(new AbortController().signal)).rejects.toMatchObject({ reason: 'too-many-tags' });
    expect(fetchMock).toHaveBeenCalledTimes(10);
  });
});
