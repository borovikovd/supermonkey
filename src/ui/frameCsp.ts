export function frameCsp(remoteImages: boolean): string {
  return remoteImages
    ? "script-src 'none'"
    : "default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:";
}
