// files.js -- localStorage-based file manager for the Wirescript playground

const STORAGE_FILES = 'ws_files';
const STORAGE_ACTIVE = 'ws_active';

const DEFAULT_FILE_NAME = 'main.ws';
const DEFAULT_FILE_CONTENT = `// Welcome to the Wirescript Playground!
// Write your Wirescript code here and hit Compile to download a .brz file.

var count: int = 0

in trigger: exec

on trigger {
  count = count + 1
}

on RoundStart {
  count = 0
}

out total = count.Value
out doubled = count.Value * 2
`;

function getFilesObject() {
  try {
    const raw = localStorage.getItem(STORAGE_FILES);
    if (raw) {
      const parsed = JSON.parse(raw);
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
        return parsed;
      }
    }
  } catch (e) {
    // corrupted storage
  }
  return null;
}

function setFilesObject(obj) {
  localStorage.setItem(STORAGE_FILES, JSON.stringify(obj));
}

function ensureDefault() {
  let files = getFilesObject();
  if (!files || Object.keys(files).length === 0) {
    files = { [DEFAULT_FILE_NAME]: DEFAULT_FILE_CONTENT };
    setFilesObject(files);
  }
  return files;
}

export function listFiles() {
  const files = ensureDefault();
  return Object.keys(files).sort();
}

export function readFile(name) {
  const files = ensureDefault();
  return files[name] !== undefined ? files[name] : null;
}

export function writeFile(name, content) {
  const files = ensureDefault();
  files[name] = content;
  setFilesObject(files);
}

export function deleteFile(name) {
  const files = ensureDefault();
  if (files[name] !== undefined) {
    delete files[name];
    setFilesObject(files);
    // If we deleted the active file, clear the active
    if (getActiveFile() === name) {
      const remaining = Object.keys(files);
      if (remaining.length > 0) {
        setActiveFile(remaining.sort()[0]);
      } else {
        localStorage.removeItem(STORAGE_ACTIVE);
      }
    }
    return true;
  }
  return false;
}

export function renameFile(oldName, newName) {
  if (oldName === newName) return true;
  const files = ensureDefault();
  if (files[oldName] === undefined) return false;
  if (files[newName] !== undefined) return false; // target exists
  files[newName] = files[oldName];
  delete files[oldName];
  setFilesObject(files);
  if (getActiveFile() === oldName) {
    setActiveFile(newName);
  }
  return true;
}

export function getActiveFile() {
  const active = localStorage.getItem(STORAGE_ACTIVE);
  const files = ensureDefault();
  // Validate active file still exists
  if (active && files[active] !== undefined) {
    return active;
  }
  // Fall back to first file
  const names = Object.keys(files).sort();
  if (names.length > 0) {
    localStorage.setItem(STORAGE_ACTIVE, names[0]);
    return names[0];
  }
  return null;
}

export function setActiveFile(name) {
  localStorage.setItem(STORAGE_ACTIVE, name);
}
