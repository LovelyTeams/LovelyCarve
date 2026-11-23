/** Paths below `cwd` that match a Bun.Glob pattern, with forward slashes and in a stable order. */
export async function globFiles(pattern: string, cwd = '.'): Promise<string[]> {
  const paths = await Array.fromAsync(new Bun.Glob(pattern).scan({ cwd }));
  return paths.map(path => path.replaceAll('\\', '/')).sort();
}
