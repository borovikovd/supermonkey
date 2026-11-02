// Fixed file contents for seeds and the file chooser, so tests can compare
// what Gmail received byte for byte.

/** Deterministic bytes: a header, then a repeating pattern up to `size`. */
function bytes(header: string, size: number, salt: number): Buffer {
  const out = Buffer.alloc(size);
  out.write(header, "latin1");
  for (let i = header.length; i < size; i++) out[i] = (i * 31 + salt) % 251;
  return out;
}

/** A 1×1 PNG. */
const PIXEL = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYPgPAAEEAQBfs347AAAAAElFTkSuQmCC",
  "base64",
);

export type FileFixture = { name: string; mimeType: string; buffer: Buffer };

export const files = {
  /** `fwd`'s attachment, 40 KB. */
  contractPdf: {
    name: "contract.pdf",
    mimeType: "application/pdf",
    buffer: bytes("%PDF-1.4\n", 40 * 1024, 1),
  },
  /** `fwd`'s inline image (Content-ID `logo`). */
  logoPng: { name: "logo.png", mimeType: "image/png", buffer: PIXEL },
  /** `dLatest`'s attachment. */
  specPdf: {
    name: "spec.pdf",
    mimeType: "application/pdf",
    buffer: bytes("%PDF-1.4\n", 12 * 1024, 2),
  },
  /** For the file chooser (ATT-01), 100 KB. */
  reportPdf: {
    name: "report.pdf",
    mimeType: "application/pdf",
    buffer: bytes("%PDF-1.4\n", 100 * 1024, 3),
  },
  photoPng: { name: "photo.png", mimeType: "image/png", buffer: PIXEL },
  notesTxt: {
    name: "notes.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("Notes for the meeting.\n"),
  },
} satisfies Record<string, FileFixture>;

/** A fixture as a control-API attachment. */
export function attachment(f: FileFixture, contentId?: string) {
  return {
    filename: f.name,
    mimeType: f.mimeType,
    content: f.buffer.toString("base64"),
    base64: true,
    ...(contentId === undefined ? {} : { contentId }),
  };
}

/** A data: URL of the pixel, for HTML that embeds an image inline. */
export const pixelDataUrl = `data:image/png;base64,${PIXEL.toString("base64")}`;
