/** The last part of a path. */
export const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

/** "512 KB", "3.4 MB". */
export const fileSize = (bytes: number) =>
  bytes >= 1e6 ? `${(bytes / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1e3))} KB`;
