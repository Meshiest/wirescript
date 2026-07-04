// share.js -- URL sharing via gzip + base64url in the hash fragment

/**
 * Compress source code and set it as the URL hash.
 * @param {string} source
 */
export async function encode(source) {
  const bytes = new TextEncoder().encode(source);
  const cs = new CompressionStream('gzip');
  const writer = cs.writable.getWriter();
  writer.write(bytes);
  writer.close();

  const chunks = [];
  const reader = cs.readable.getReader();
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
  }

  const totalLen = chunks.reduce((s, c) => s + c.length, 0);
  const compressed = new Uint8Array(totalLen);
  let offset = 0;
  for (const chunk of chunks) {
    compressed.set(chunk, offset);
    offset += chunk.length;
  }

  // base64url encode (no padding)
  const base64 = btoa(String.fromCharCode(...compressed))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '');

  window.history.replaceState(null, '', '#' + base64);
}

/**
 * Read the URL hash and decompress it back to source code.
 * @returns {Promise<string|null>}
 */
export async function decode() {
  const hash = window.location.hash.slice(1);
  if (!hash) return null;

  try {
    // base64url decode
    const base64 = hash.replace(/-/g, '+').replace(/_/g, '/');
    // Add padding back
    const padded = base64 + '='.repeat((4 - (base64.length % 4)) % 4);
    const binaryStr = atob(padded);
    const compressed = new Uint8Array(binaryStr.length);
    for (let i = 0; i < binaryStr.length; i++) {
      compressed[i] = binaryStr.charCodeAt(i);
    }

    const ds = new DecompressionStream('gzip');
    const writer = ds.writable.getWriter();
    writer.write(compressed);
    writer.close();

    const chunks = [];
    const reader = ds.readable.getReader();
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
    }

    const totalLen = chunks.reduce((s, c) => s + c.length, 0);
    const decompressed = new Uint8Array(totalLen);
    let offset = 0;
    for (const chunk of chunks) {
      decompressed.set(chunk, offset);
      offset += chunk.length;
    }

    return new TextDecoder().decode(decompressed);
  } catch (e) {
    console.warn('Failed to decode shared URL:', e);
    return null;
  }
}
