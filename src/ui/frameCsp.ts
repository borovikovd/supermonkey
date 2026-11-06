// The Content-Security-Policy of a frame that shows mail HTML: the message
// frame and the composer's quoted text.

/**
 * Scripts never run. With remote images off nothing outside the message loads
 * either: no images, web fonts, or stylesheets, since any of them tells the
 * sender the mail was opened.
 */
export function frameCsp(remoteImages: boolean): string {
  return remoteImages
    ? "script-src 'none'"
    : "default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:";
}
