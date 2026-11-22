export type MimePart = {
  headers: Record<string, string>;
  contentType: string;
  params: Record<string, string>;
  contentId: string | null;
  disposition: string | null;
  filename: string | null;
  body: Buffer;
  parts: MimePart[];
};

export function decodeWords(value: string): string {
  return value
    .replace(/\?=\s+=\?/g, "?==?")
    .replace(
      /=\?([^?]+)\?([BbQq])\?([^?]*)\?=/g,
      (_m, _charset: string, enc: string, text: string) =>
        enc.toUpperCase() === "B"
          ? Buffer.from(text, "base64").toString("utf8")
          : decodeQuotedPrintable(text.replace(/_/g, " ")).toString("utf8"),
    );
}

function decodeQuotedPrintable(text: string): Buffer {
  const bytes: number[] = [];
  const s = text.replace(/=\r?\n/g, "");
  for (let i = 0; i < s.length; i++) {
    const hex = s.slice(i + 1, i + 3);
    if (s[i] === "=" && /^[0-9A-Fa-f]{2}$/.test(hex)) {
      bytes.push(Number.parseInt(hex, 16));
      i += 2;
    } else {
      bytes.push(...Buffer.from(s.charAt(i), "utf8"));
    }
  }
  return Buffer.from(bytes);
}

function splitHead(raw: string): [string, string] {
  const m = /\r?\n\r?\n/.exec(raw);
  return m ? [raw.slice(0, m.index), raw.slice(m.index + m[0].length)] : [raw, ""];
}

function parseHeaders(head: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of head.replace(/\r?\n[ \t]+/g, " ").split(/\r?\n/)) {
    const colon = line.indexOf(":");
    if (colon <= 0) continue;
    const name = line.slice(0, colon).trim().toLowerCase();
    out[name] ??= decodeWords(line.slice(colon + 1).trim());
  }
  return out;
}

function parseValue(value: string): [string, Record<string, string>] {
  const [first = "", ...rest] = value.split(";");
  const params: Record<string, string> = {};
  for (const p of rest) {
    const eq = p.indexOf("=");
    if (eq > 0) {
      params[p.slice(0, eq).trim().toLowerCase()] = p
        .slice(eq + 1)
        .trim()
        .replace(/^"|"$/g, "");
    }
  }
  return [first.trim().toLowerCase(), params];
}

function decodeBody(body: string, encoding: string): Buffer {
  switch (encoding.toLowerCase()) {
    case "base64":
      return Buffer.from(body.replace(/\s+/g, ""), "base64");
    case "quoted-printable":
      return decodeQuotedPrintable(body);
    default:
      return Buffer.from(body, "utf8");
  }
}

export function parseMime(raw: string): MimePart {
  const [head, body] = splitHead(raw);
  const headers = parseHeaders(head);
  const [contentType, params] = parseValue(headers["content-type"] ?? "text/plain");
  const dispositionHeader = headers["content-disposition"];
  const [disposition, dispParams]: [string | null, Record<string, string>] = dispositionHeader
    ? parseValue(dispositionHeader)
    : [null, {}];
  const part: MimePart = {
    headers,
    contentType,
    params,
    contentId: headers["content-id"]?.replace(/^<|>$/g, "") ?? null,
    disposition,
    filename: dispParams.filename ?? params.name ?? null,
    body: Buffer.alloc(0),
    parts: [],
  };
  const boundary = params.boundary;
  if (contentType.startsWith("multipart/") && boundary) {
    const delimiter = `--${boundary}`;
    const chunks = body.split(new RegExp(`\\r?\\n?${escapeRegExp(delimiter)}`));
    for (const chunk of chunks.slice(1)) {
      if (chunk.startsWith("--")) break;
      part.parts.push(parseMime(chunk.replace(/^[ \t]*\r?\n/, "")));
    }
  } else {
    part.body = decodeBody(body, headers["content-transfer-encoding"] ?? "7bit");
  }
  return part;
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export function leaves(part: MimePart): MimePart[] {
  return part.parts.length === 0 ? [part] : part.parts.flatMap(leaves);
}

export function findPart(
  part: MimePart,
  pred: string | ((p: MimePart) => boolean),
): MimePart | undefined {
  const test = typeof pred === "string" ? (p: MimePart) => p.contentType === pred : pred;
  return [part, ...part.parts.flatMap(allParts)].find(test);
}

function allParts(part: MimePart): MimePart[] {
  return [part, ...part.parts.flatMap(allParts)];
}

export function shape(part: MimePart): string {
  return part.parts.length === 0
    ? part.contentType
    : `${part.contentType}[${part.parts.map(shape).join(",")}]`;
}

export function htmlOf(part: MimePart): string {
  return (findPart(part, "text/html") ?? findPart(part, "text/plain"))?.body.toString("utf8") ?? "";
}

export function attachments(part: MimePart): MimePart[] {
  return leaves(part).filter((p) => p.filename !== null);
}
