const TAGS_ENDPOINT = 'https://api.github.com/repos/LovelyTeams/LovelyCarve/tags';
export const TAGS_PAGE_URL = 'https://github.com/LovelyTeams/LovelyCarve/tags';

export class TagCheckError extends Error {
  constructor(public readonly reason: 'rate-limit' | 'response' | 'too-many-tags') {
    super(reason);
  }
}

/** Read only public tags; never send credentials or follow URLs supplied by the response. */
export async function fetchVersionTags(signal: AbortSignal): Promise<string[]> {
  const tags: string[] = [];
  for (let page = 1; page <= 10; page++) {
    const response = await fetch(`${TAGS_ENDPOINT}?per_page=100&page=${page}`, {
      signal,
      credentials: 'omit',
      referrerPolicy: 'no-referrer',
      redirect: 'error',
      headers: { Accept: 'application/vnd.github+json' },
    });
    if (response.status === 403 || response.status === 429) throw new TagCheckError('rate-limit');
    if (!response.ok) throw new TagCheckError('response');
    const entries: unknown = await response.json();
    if (!Array.isArray(entries) || entries.length > 100 || entries.some(entry =>
      !entry || typeof entry.name !== 'string' || entry.name.length > 256)) {
      throw new TagCheckError('response');
    }
    tags.push(...entries.map(entry => entry.name as string));
    if (entries.length < 100) return tags;
  }
  // An incomplete list cannot establish that the installed version is current.
  throw new TagCheckError('too-many-tags');
}
