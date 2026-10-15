// share.js -- URL sharing via gzip + base64url in the hash fragment
//
// Two hash forms. A single source is the bare base64url text. A set of files
// is `2.` followed by the base64url of `{ files: { "name.ws": text, ... },
// open: "name.ws" }`; `.` is never a base64url character, so the two cannot be
// confused.

const FILES_PREFIX = '2.';

/** A name a shared file may have: a `.ws` file, no folders. */
const isFileName = (name) => /^[\w .-]+\.ws$/.test(name) && !name.startsWith('.');

async function pipe(bytes, stream) {
  const writer = stream.writable.getWriter();
  // A bad input fails these too, but the reader below reports the same error.
  writer.write(bytes).catch(() => {});
  writer.close().catch(() => {});
  const chunks = [];
  const reader = stream.readable.getReader();
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
  }
  const out = new Uint8Array(chunks.reduce((s, c) => s + c.length, 0));
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

/**
 * `text`, gzipped and base64url encoded (no padding).
 * @param {string} text
 * @returns {Promise<string>}
 */
export async function encodeToString(text) {
  const compressed = await pipe(new TextEncoder().encode(text), new CompressionStream('gzip'));
  let binary = '';
  for (let i = 0; i < compressed.length; i++) binary += String.fromCharCode(compressed[i]);
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/**
 * The text `encodeToString` encoded.
 * @param {string} encoded
 * @returns {Promise<string>}
 */
export async function decodeString(encoded) {
  const base64 = encoded.replace(/-/g, '+').replace(/_/g, '/');
  const binaryStr = atob(base64 + '='.repeat((4 - (base64.length % 4)) % 4));
  const compressed = new Uint8Array(binaryStr.length);
  for (let i = 0; i < binaryStr.length; i++) compressed[i] = binaryStr.charCodeAt(i);
  return new TextDecoder().decode(await pipe(compressed, new DecompressionStream('gzip')));
}

/**
 * The hash (without `#`) that opens `files` in the playground, with `open`
 * as the active file.
 * @param {Record<string, string>} files
 * @param {string} open
 * @returns {Promise<string>}
 */
export async function encodeFiles(files, open) {
  return FILES_PREFIX + (await encodeToString(JSON.stringify({ files, open })));
}

/**
 * Compress source code and set it as the URL hash.
 * @param {string} source
 */
export async function encode(source) {
  window.history.replaceState(null, '', '#' + (await encodeToString(source)));
}

/**
 * Read a hash: a single source as a string, a set of files as
 * `{ files, open }`, or null when there is none or it cannot be read.
 * @param {string} hash the hash without `#`
 * @returns {Promise<string|{files: Record<string, string>, open: string}|null>}
 */
export async function decodeHash(hash) {
  if (!hash) return null;
  try {
    if (!hash.startsWith(FILES_PREFIX)) return await decodeString(hash);
    // The hash is anyone's link, so only a plain set of `.ws` text files is
    // accepted; nothing named like an object built-in reaches storage.
    const payload = JSON.parse(await decodeString(hash.slice(FILES_PREFIX.length)));
    const raw = payload && payload.files;
    if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null;
    const files = {};
    for (const [name, text] of Object.entries(raw)) {
      if (!isFileName(name) || typeof text !== 'string') return null;
      files[name] = text;
    }
    const names = Object.keys(files);
    if (names.length === 0) return null;
    const open = typeof payload.open === 'string' && Object.hasOwn(files, payload.open) ? payload.open : names[0];
    return { files, open };
  } catch (e) {
    console.warn('Failed to decode shared URL:', e);
    return null;
  }
}

/**
 * Read the URL hash; see `decodeHash`.
 * @returns {Promise<string|{files: Record<string, string>, open: string}|null>}
 */
export async function decode() {
  return decodeHash(window.location.hash.slice(1));
}
