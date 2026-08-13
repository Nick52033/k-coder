export function toUserFacingPath(value: string): string {
  if (/^\\\\\?\\UNC\\/i.test(value)) {
    return `\\\\${value.slice(8)}`;
  }
  if (/^\\\\\?\\/.test(value)) {
    return value.slice(4);
  }
  return value;
}

function normalizeWorkspacePath(value: string): string {
  return toUserFacingPath(value)
    .replace(/\\/g, "/")
    .replace(/\/$/, "");
}

function isWindowsHost(): boolean {
  return typeof navigator !== "undefined" && navigator.userAgent.includes("Windows");
}

export function workspacePathKey(value: string): string {
  const normalized = normalizeWorkspacePath(value);
  return isWindowsHost() ? normalized.toLocaleLowerCase() : normalized;
}

export function workspacePathsEqual(left: string, right: string): boolean {
  return workspacePathKey(left) === workspacePathKey(right);
}
