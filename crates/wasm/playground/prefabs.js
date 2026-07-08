// prefabs.js -- uploaded .brz prefab store for the Wirescript playground.
//
// Binary prefabs can't live in localStorage, so we keep the uploaded files as
// Blobs in IndexedDB (persistent across reloads) plus an in-memory byte cache
// so the compiler/completion (which call synchronously into wasm) can read the
// bytes without awaiting. Each prefab is keyed by its wirescript reference form
// `./name.brz` -- the exact string the `$./name.brz` literal resolves to.

const DB_NAME = 'ws_prefabs';
const STORE = 'blobs';

/** ref ("./name.brz") -> Uint8Array */
let cache = new Map();

function openDb() {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, 1);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE)) db.createObjectStore(STORE);
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function reqPromise(req) {
  return new Promise((res, rej) => {
    req.onsuccess = () => res(req.result);
    req.onerror = () => rej(req.error);
  });
}

function txDone(tx) {
  return new Promise((res, rej) => {
    tx.oncomplete = () => res();
    tx.onerror = () => rej(tx.error);
    tx.onabort = () => rej(tx.error);
  });
}

/** Normalize an uploaded file name to a `./name.brz` reference (basename only). */
export function normalizeRef(name) {
  const base = String(name).replace(/^.*[\\/]/, '');
  return './' + base;
}

/**
 * Load every persisted prefab blob into the in-memory byte cache. Call once at
 * startup. Safe to call when IndexedDB is unavailable (private mode) -- the
 * cache just stays empty and uploads work for the session only.
 */
export async function initPrefabs() {
  try {
    const db = await openDb();
    const tx = db.transaction(STORE, 'readonly');
    const store = tx.objectStore(STORE);
    const refs = await reqPromise(store.getAllKeys());
    const blobs = await reqPromise(store.getAll());
    const next = new Map();
    for (let i = 0; i < refs.length; i++) {
      const buf = await blobs[i].arrayBuffer();
      next.set(String(refs[i]), new Uint8Array(buf));
    }
    cache = next;
  } catch (e) {
    console.warn('prefab store init failed; uploads will not persist', e);
    cache = new Map();
  }
}

/** Add (or replace) a prefab from a File/Blob. Returns its `./name.brz` ref. */
export async function addPrefab(file) {
  const ref = normalizeRef(file.name || 'prefab.brz');
  const buf = await file.arrayBuffer();
  cache.set(ref, new Uint8Array(buf));
  try {
    const db = await openDb();
    const tx = db.transaction(STORE, 'readwrite');
    tx.objectStore(STORE).put(new Blob([buf]), ref);
    await txDone(tx);
  } catch (e) {
    console.warn('prefab persist failed', e);
  }
  return ref;
}

export async function deletePrefab(ref) {
  cache.delete(ref);
  try {
    const db = await openDb();
    const tx = db.transaction(STORE, 'readwrite');
    tx.objectStore(STORE).delete(ref);
    await txDone(tx);
  } catch (e) {
    /* ignore */
  }
}

/** Sorted list of `./name.brz` refs currently registered. */
export function listPrefabs() {
  return Array.from(cache.keys()).sort();
}

export function prefabSize(ref) {
  const b = cache.get(ref);
  return b ? b.length : 0;
}

/**
 * Full registry as the JSON the wasm resolver expects:
 * `{ "./name.brz": [byte, ...] }`. Used at compile so `$./name.brz` embeds.
 */
export function getPrefabsJson() {
  const out = {};
  for (const [ref, bytes] of cache) out[ref] = Array.from(bytes);
  return JSON.stringify(out);
}

/**
 * Keys-only registry (`{ "./name.brz": [] }`) -- cheap to build on every
 * keystroke; drives `$./` completion, which only needs the paths.
 */
export function getPrefabPathsJson() {
  const out = {};
  for (const ref of cache.keys()) out[ref] = [];
  return JSON.stringify(out);
}
