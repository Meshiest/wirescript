// `match` on single values: literal, range, string-pattern, alternative and
// guarded arms. Paste into a grid; on load it prints `match values: N/N` and a
// FAIL line for each check that disagrees.
//
// Each pure check is a differential: the same `match` body runs once through
// the compile-time evaluator (a `const mod`, its result passed to a `const`
// parameter) and once as gates (a plain `mod`). `@nofold` keeps the gate half
// from being folded at compile time, so the game evaluates it. The statement
// checks compare against the value written next to them.
@nofold

mod checkInt(pass: *int, total: *int, cv: const int, rv: int, label: string) {
  total = total + 1
  let ok = cv == rv
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: ${label}: want ${cv}, got ${rv}") }
}

mod checkStr(pass: *int, total: *int, cv: const string, rv: string, label: string) {
  total = total + 1
  let ok = cv == rv
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: ${label}: want '${cv}', got '${rv}'") }
}

const mod cOp(c: string) -> int {
  return match c { "+" => 1, "-" | "<" => 2, "" => 3, _ => 4 }
}

mod rOp(c: string) -> int {
  return match c { "+" => 1, "-" | "<" => 2, "" => 3, _ => 4 }
}

const mod cCase(c: string) -> int {
  return match c { "a" => 1, _ => 2 }
}

mod rCase(c: string) -> int {
  return match c { "a" => 1, _ => 2 }
}

const mod cCut(c: string) -> string {
  return match c {
    "say " .. r => r,
    name .. ".ws" => name,
    k .. "=" .. v if k != "" => k .. "|" .. v,
    "<" .. inner .. ">" => "[" .. inner .. "]",
    _ => "?",
  }
}

mod rCut(c: string) -> string {
  return match c {
    "say " .. r => r,
    name .. ".ws" => name,
    k .. "=" .. v if k != "" => k .. "|" .. v,
    "<" .. inner .. ">" => "[" .. inner .. "]",
    _ => "?",
  }
}

const mod cSize(x: int) -> string {
  return match x { 0 => "zero", 1..10 => "few", v if v >= 100 => "lots", _ => "some" }
}

mod rSize(x: int) -> string {
  return match x { 0 => "zero", 1..10 => "few", v if v >= 100 => "lots", _ => "some" }
}

const mod cTag(t: int) -> int {
  return match t { :red | :blue => 1, :green => 2, _ => 3 }
}

mod rTag(t: int) -> int {
  return match t { :red | :blue => 1, :green => 2, _ => 3 }
}

const mod cHalf(f: float) -> int {
  return match f { 0 => 0, 0.5 => 1, 1..2 => 2, _ => 3 }
}

mod rHalf(f: float) -> int {
  return match f { 0 => 0, 0.5 => 1, 1..2 => 2, _ => 3 }
}

// Two middle literals: the second `Find` starts one past the first match, so
// this checks the game's Find start is 0-based. "aba" has both ends but no
// room for them, and "plain" has no "-" at all (Find gives -1).
const mod cParts(s: string) -> string {
  return match s {
    "${a}-${b}-${c}" => c .. b .. a,
    "ab" .. x .. "ba" => "[" .. x .. "]",
    _ => "?",
  }
}

mod rParts(s: string) -> string {
  return match s {
    "${a}-${b}-${c}" => c .. b .. a,
    "ab" .. x .. "ba" => "[" .. x .. "]",
    _ => "?",
  }
}

const mod cNeg(x: int) -> string {
  return match x { -1 => "minus one", -5..0 => "negative", _ => "other" }
}

mod rNeg(x: int) -> string {
  return match x { -1 => "minus one", -5..0 => "negative", _ => "other" }
}

const mod cBool(b: bool) -> int {
  return match b { true => 1, false => 0 }
}

mod rBool(b: bool) -> int {
  return match b { true => 1, false => 0 }
}

enum Cmd { Say(string), Move(int), Wait, Quit }

var pass: int = 0
var total: int = 0
var said: string = ""
var hits: int = 0
var cmd: Cmd = Cmd.Wait

// A literal and a string pattern inside a stored enum's payload, a guard on
// it, and `|` over two variants.
mod cmdKind() -> int {
  return match cmd {
    Move(0) => 1,
    Move(n) if n > 5 => 2,
    Move(_) => 3,
    Say("/" .. c) => 40 + c.Length(),
    Say(_) => 5,
    Wait | Quit => 6,
  }
}

on Loaded() {
  checkInt(pass, total, cOp("+"), rOp("+"), "op +")
  checkInt(pass, total, cOp("-"), rOp("-"), "op -")
  checkInt(pass, total, cOp("<"), rOp("<"), "op <")
  checkInt(pass, total, cOp(""), rOp(""), "op empty")
  checkInt(pass, total, cOp("++"), rOp("++"), "op ++ (not +)")
  checkInt(pass, total, cOp("P"), rOp("P"), "op P")
  // A literal arm compares with the game's string ==; this fails if that
  // ignores case.
  checkInt(pass, total, cCase("A"), rCase("A"), "literal arm is case-sensitive")

  checkStr(pass, total, cCut("say hi"), rCut("say hi"), "say prefix")
  checkStr(pass, total, cCut("say "), rCut("say "), "say empty rest")
  checkStr(pass, total, cCut("Say hi"), rCut("Say hi"), "say is case-sensitive")
  checkStr(pass, total, cCut("main.ws"), rCut("main.ws"), "suffix")
  checkStr(pass, total, cCut("a.ws.ws"), rCut("a.ws.ws"), "suffix ends the string")
  checkStr(pass, total, cCut("a=b=c"), rCut("a=b=c"), "split at the first =")
  checkStr(pass, total, cCut("=b"), rCut("=b"), "guard false falls through")
  checkStr(pass, total, cCut("<x>"), rCut("<x>"), "both ends")
  checkStr(pass, total, cCut("<>"), rCut("<>"), "both ends, empty middle")
  checkStr(pass, total, cCut(">"), rCut(">"), "both ends need two chars")
  checkStr(pass, total, cCut("éa.ws"), rCut("éa.ws"), "code points")

  checkStr(pass, total, cSize(0), rSize(0), "size 0")
  checkStr(pass, total, cSize(1), rSize(1), "size 1 (range start)")
  checkStr(pass, total, cSize(9), rSize(9), "size 9")
  checkStr(pass, total, cSize(10), rSize(10), "size 10 (range end is out)")
  checkStr(pass, total, cSize(99), rSize(99), "size 99")
  checkStr(pass, total, cSize(100), rSize(100), "size 100 (guard)")
  checkStr(pass, total, cSize(-5), rSize(-5), "size -5")

  checkInt(pass, total, cTag(:red), rTag(:red), "atom red")
  checkInt(pass, total, cTag(:blue), rTag(:blue), "atom blue")
  checkInt(pass, total, cTag(:green), rTag(:green), "atom green")
  checkInt(pass, total, cTag(7), rTag(7), "atom other")

  checkInt(pass, total, cHalf(0.0), rHalf(0.0), "float 0")
  checkInt(pass, total, cHalf(0.5), rHalf(0.5), "float 0.5")
  checkInt(pass, total, cHalf(1.5), rHalf(1.5), "float in range")
  checkInt(pass, total, cHalf(2.0), rHalf(2.0), "float range end")

  // Statement form: exactly one arm runs.
  said = "say hello"
  match said {
    "say " .. words => {
      hits += 1
      said = words
    }
    _ => { hits += 100 }
  }
  checkStr(pass, total, "hello", said, "statement binds the hole")
  checkInt(pass, total, 1, hits, "statement runs one arm")

  // `let ... else`: a match continues, a miss runs the else.
  said = "go north"
  let "go " .. place = said else {
    BroadcastChatMessage("FAIL: let else missed")
    return
  }
  checkStr(pass, total, "north", place, "let else binds")

  checkStr(pass, total, cParts("x-y-z"), rParts("x-y-z"), "two middle literals")
  checkStr(pass, total, cParts("x-y-z-w"), rParts("x-y-z-w"), "last hole takes the rest")
  checkStr(pass, total, cParts("x-y"), rParts("x-y"), "second literal missing")
  checkStr(pass, total, cParts("plain"), rParts("plain"), "Find finds nothing")
  checkStr(pass, total, cParts("abba"), rParts("abba"), "both ends, nothing between")
  checkStr(pass, total, cParts("abXba"), rParts("abXba"), "both ends")
  checkStr(pass, total, cParts("aba"), rParts("aba"), "both ends overlap: no room")

  checkStr(pass, total, cNeg(-1), rNeg(-1), "negative literal")
  checkStr(pass, total, cNeg(-3), rNeg(-3), "negative range")
  checkStr(pass, total, cNeg(0), rNeg(0), "negative range end is out")

  checkInt(pass, total, cBool(true), rBool(true), "bool true")
  checkInt(pass, total, cBool(false), rBool(false), "bool false")

  // A guard in the statement form, falling through when false.
  said = "go"
  hits = 0
  match said {
    w if w.Length() > 5 => { hits += 1 }
    "go" => { hits += 10 }
    _ => { hits += 100 }
  }
  checkInt(pass, total, 10, hits, "statement guard falls through")

  // `if let` takes both paths.
  said = "tp home"
  if let "tp ${where}" = said { hits = where.Length() } else { hits = -1 }
  checkInt(pass, total, 4, hits, "if let matches")
  said = "teleport"
  if let "tp ${where}" = said { hits = where.Length() } else { hits = -1 }
  checkInt(pass, total, -1, hits, "if let misses")

  // The `${}` spelling in `let ... else`.
  said = "key:value"
  let "${k}:${v}" = said else {
    BroadcastChatMessage("FAIL: template let else missed")
    return
  }
  checkStr(pass, total, "value=key", v .. "=" .. k, "template let else binds")

  cmd = Cmd.Move(0)
  checkInt(pass, total, 1, cmdKind(), "payload literal")
  cmd = Cmd.Move(9)
  checkInt(pass, total, 2, cmdKind(), "payload guard")
  cmd = Cmd.Move(3)
  checkInt(pass, total, 3, cmdKind(), "payload guard falls through")
  cmd = Cmd.Say("/kick")
  checkInt(pass, total, 44, cmdKind(), "payload string pattern")
  cmd = Cmd.Say("hi")
  checkInt(pass, total, 5, cmdKind(), "payload string pattern misses")
  cmd = Cmd.Quit
  checkInt(pass, total, 6, cmdKind(), "variant alternatives")

  BroadcastChatMessage("match values: ${pass}/${total}")
}
