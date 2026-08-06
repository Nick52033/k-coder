export type ComposerTriggerKind = "file" | "skill";

export interface ComposerTrigger {
  kind: ComposerTriggerKind;
  start: number;
  end: number;
  query: string;
}

/** Find the @file or /skill token around the textarea caret. */
export function findComposerTrigger(value: string, cursor: number): ComposerTrigger | null {
  const safeCursor = Math.max(0, Math.min(cursor, value.length));
  let start = safeCursor;
  while (start > 0 && !/\s/.test(value[start - 1])) start -= 1;
  const marker = value[start];
  if (marker !== "@" && marker !== "/") return null;
  if (start > 0 && !/\s/.test(value[start - 1])) return null;

  let end = safeCursor;
  while (end < value.length && !/\s/.test(value[end])) end += 1;
  const query = value.slice(start + 1, safeCursor);
  if (query.includes("\n") || query.includes("\r")) return null;
  return { kind: marker === "@" ? "file" : "skill", start, end, query };
}
