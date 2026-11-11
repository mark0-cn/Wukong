/** Presentation only. Keep the original path for native calls and identity checks. */
export function displayPath(path: string): string {
  if (/^\\\\\?\\[a-z]:\\/i.test(path)) return path.slice(4);
  if (path.startsWith('\\\\?\\UNC\\')) return `\\\\${path.slice(8)}`;
  return path;
}
