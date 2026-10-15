// Escape markup in a display name before showing it. A `mod`, so each use is a single gate.
mod escape(text: string) -> string {
  return text.Replace("<", "&lt;")
}

// Clean free-form player input: trim, cap the length, then escape.
// Capping first keeps the cut from landing inside an `&lt;`.
fn sanitize(raw: string, maxLen: int = 24) -> string {
  return raw.Trim().Substring(0, maxLen).Replace("<", "&lt;")
}
