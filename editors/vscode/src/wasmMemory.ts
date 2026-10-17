/**
 * The page limits of the memory a WASI module imports. The JS API hides them,
 * and a threaded module needs its shared memory created to match, so they are
 * read from the binary's import section.
 */
export function importedMemoryLimits(bytes: Uint8Array): { initial: number; maximum: number } {
  let p = 8; // past the magic number and version
  const leb = (): number => {
    let result = 0;
    let shift = 0;
    let b: number;
    do {
      b = bytes[p++];
      result += (b & 0x7f) * 2 ** shift;
      shift += 7;
    } while (b & 0x80);
    return result;
  };
  const skipName = () => {
    const n = leb();
    p += n;
  };
  while (p < bytes.length) {
    const id = bytes[p++];
    const size = leb();
    const end = p + size;
    if (id !== 2) {
      p = end;
      continue;
    }
    for (let count = leb(); count > 0; count--) {
      skipName();
      skipName();
      const kind = bytes[p++];
      if (kind === 0) {
        leb(); // function: type index
      } else if (kind === 1) {
        p++; // table: element type
        const flags = leb();
        leb();
        if (flags & 1) leb();
      } else if (kind === 3) {
        p += 2; // global: value type, mutability
      } else if (kind === 2) {
        const flags = leb();
        const initial = leb();
        return { initial, maximum: flags & 1 ? leb() : initial };
      }
    }
    p = end;
  }
  throw new Error("the WASI module imports no memory");
}
