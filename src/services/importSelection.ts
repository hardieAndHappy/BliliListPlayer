import type { ParsedItem } from '../types/playlist';

export function dedupAgainstExisting(newItems: ParsedItem[], existingIds: string[]): ParsedItem[] {
  const existing = new Set(existingIds.map((id) => id.toLowerCase()));
  return newItems.filter((item) => !existing.has(item.id.toLowerCase()));
}

export function toggleSelect(selected: Set<string>, id: string): Set<string> {
  const next = new Set(selected);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
}

export function selectAll(items: ParsedItem[], existingIds: string[]): Set<string> {
  const existing = new Set(existingIds.map((id) => id.toLowerCase()));
  const result = new Set<string>();
  for (const item of items) {
    if (!existing.has(item.id.toLowerCase())) result.add(item.id);
  }
  return result;
}
