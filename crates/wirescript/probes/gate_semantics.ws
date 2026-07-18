/// Gate semantics probe — prints one CASE line per observed interaction.
/// Paste into a local world; it runs itself (ReadBrickGrid fires on paste).
/// Bump VERSION whenever the case matrix changes.
let VERSION = 2

let grid = ReadBrickGrid()

// A top-level `in` port that nothing ever wires. Reading it observes how a
// gate behaves when one of its inputs is left unconnected (its "unwired"
// resting value).
in neverWired: int

mod caseLine(msg: string) {
  PrintToConsole(msg)
  // Paced well below any console/log rate limit: both truncated runs died
  // after a burst of fast prints, so give the log room to breathe.
  await SleepTicks(_, delay = 5)
}

// -- Eq family: CompareEqual / CompareNotEqual, every variant pairing --------
mod eqII(a: int, b: int) {
  caseLine("CASE CompareEqual int:${a} int:${b} -> ${Opaque(a) == Opaque(b)}")
  caseLine("CASE CompareNotEqual int:${a} int:${b} -> ${Opaque(a) != Opaque(b)}")
}
mod eqIS(a: int, b: string) {
  caseLine("CASE CompareEqual int:${a} str:${b} -> ${Opaque(a) == Opaque(b)}")
  caseLine("CASE CompareNotEqual int:${a} str:${b} -> ${Opaque(a) != Opaque(b)}")
}
mod eqIF(a: int, b: float) {
  caseLine("CASE CompareEqual int:${a} float:${b} -> ${Opaque(a) == Opaque(b)}")
}
mod eqIB(a: int, b: bool) {
  caseLine("CASE CompareEqual int:${a} bool:${b} -> ${Opaque(a) == Opaque(b)}")
}
mod eqSS(a: string, b: string) {
  caseLine("CASE CompareEqual str:${a} str:${b} -> ${Opaque(a) == Opaque(b)}")
}
mod eqFF(a: float, b: float) {
  caseLine("CASE CompareEqual float:${a} float:${b} -> ${Opaque(a) == Opaque(b)}")
}
// Labeled variants: FormatText renders NaN/inf as 0 and giant ints with
// thousands separators, so lossy values carry hardcoded label text.
mod eqFFL(la: string, a: float, lb: string, b: float) {
  caseLine("CASE CompareEqual float:${la} float:${lb} -> ${Opaque(a) == Opaque(b)}")
}
mod eqIIL(la: string, a: int, lb: string, b: int) {
  caseLine("CASE CompareEqual int:${la} int:${lb} -> ${Opaque(a) == Opaque(b)}")
  caseLine("CASE CompareNotEqual int:${la} int:${lb} -> ${Opaque(a) != Opaque(b)}")
}
mod eqBB(a: bool, b: bool) {
  caseLine("CASE CompareEqual bool:${a} bool:${b} -> ${Opaque(a) == Opaque(b)}")
}

mod runEq() {
  caseLine("BEGIN eq 40")
  eqII(0, 0)   eqII(1, 1)   eqII(1, 0)   eqII(-1, 1)
  eqIIL("9007199254740993", 9007199254740993, "9007199254740992", 9007199254740992)
  eqIS(1, "1") eqIS(0, "")  eqIS(0, "0") eqIS(1, "a") eqIS(1, "1.0")
  eqIF(1, 1.0) eqIF(0, 0.0) eqIF(1, 0.5) eqIF(-1, -1.0)
  eqIB(1, true) eqIB(0, false) eqIB(-1, true) eqIB(0, true)
  eqSS("", "") eqSS("a", "a") eqSS("a", "A") eqSS("1", "1.0")
  eqFF(0.0, 0.0) eqFF(0.5, 0.5)
  eqFFL("NaN", 0.0 / 0.0, "NaN", 0.0 / 0.0) eqFFL("inf", 1.0 / 0.0, "inf", 1.0 / 0.0)
  eqBB(true, true) eqBB(false, false) eqBB(true, false)
  caseLine("CASE CompareEqual int:1 unwired -> ${Opaque(1) == neverWired}")
  caseLine("END eq")
}

// -- Bool family: LogicalAND/OR/NOT/XOR, incl. non-bool operands -------------
// `^^` is the dedicated Logical XOR operator (lowers directly to the
// Expr_LogicalXOR gate — confirmed against catalog/operators.rs), so it is
// used here instead of `!=` (which lowers to CompareNotEqual).
mod andBB(a: bool, b: bool) {
  caseLine("CASE LogicalAND bool:${a} bool:${b} -> ${Opaque(a) && Opaque(b)}")
  caseLine("CASE LogicalOR bool:${a} bool:${b} -> ${Opaque(a) || Opaque(b)}")
  caseLine("CASE LogicalXOR bool:${a} bool:${b} -> ${Opaque(a) ^^ Opaque(b)}")
}
mod andIB(a: int, b: bool) {
  caseLine("CASE LogicalAND int:${a} bool:${b} -> ${Opaque(a) && Opaque(b)}")
  caseLine("CASE LogicalOR int:${a} bool:${b} -> ${Opaque(a) || Opaque(b)}")
}
mod andSB(a: string, b: bool) {
  caseLine("CASE LogicalAND str:${a} bool:${b} -> ${Opaque(a) && Opaque(b)}")
}
mod andFB(a: float, b: bool) {
  caseLine("CASE LogicalAND float:${a} bool:${b} -> ${Opaque(a) && Opaque(b)}")
}
mod andFBL(la: string, a: float, b: bool) {
  caseLine("CASE LogicalAND float:${la} bool:${b} -> ${Opaque(a) && Opaque(b)}")
}
mod notV(label: string, v: bool) {
  caseLine("CASE LogicalNOT bool:${v} - -> ${!Opaque(v)}")
}

mod runBool() {
  caseLine("BEGIN bool 29")
  andBB(true, true) andBB(true, false) andBB(false, true) andBB(false, false)
  andIB(0, true) andIB(1, true) andIB(-1, true) andIB(2, false)
  andSB("", true) andSB("a", true) andSB("0", true)
  andFB(0.0, true) andFB(0.5, true) andFBL("NaN", 0.0 / 0.0, true)
  notV("t", true) notV("f", false)
  caseLine("CASE LogicalAND bool:true unwired -> ${Opaque(true) && neverWired}")
  caseLine("END bool")
}

// -- Compare family: CompareLess/LessEqual/Greater/GreaterEqual --------------
mod cmpII(a: int, b: int) {
  caseLine("CASE CompareLess int:${a} int:${b} -> ${Opaque(a) < Opaque(b)}")
  caseLine("CASE CompareLessEqual int:${a} int:${b} -> ${Opaque(a) <= Opaque(b)}")
  caseLine("CASE CompareGreater int:${a} int:${b} -> ${Opaque(a) > Opaque(b)}")
  caseLine("CASE CompareGreaterEqual int:${a} int:${b} -> ${Opaque(a) >= Opaque(b)}")
}
mod cmpFF(a: float, b: float) {
  caseLine("CASE CompareLess float:${a} float:${b} -> ${Opaque(a) < Opaque(b)}")
  caseLine("CASE CompareLessEqual float:${a} float:${b} -> ${Opaque(a) <= Opaque(b)}")
  caseLine("CASE CompareGreater float:${a} float:${b} -> ${Opaque(a) > Opaque(b)}")
  caseLine("CASE CompareGreaterEqual float:${a} float:${b} -> ${Opaque(a) >= Opaque(b)}")
}
mod cmpFFL(la: string, a: float, lb: string, b: float) {
  caseLine("CASE CompareLess float:${la} float:${lb} -> ${Opaque(a) < Opaque(b)}")
  caseLine("CASE CompareLessEqual float:${la} float:${lb} -> ${Opaque(a) <= Opaque(b)}")
  caseLine("CASE CompareGreater float:${la} float:${lb} -> ${Opaque(a) > Opaque(b)}")
  caseLine("CASE CompareGreaterEqual float:${la} float:${lb} -> ${Opaque(a) >= Opaque(b)}")
}
mod cmpIF(a: int, b: float) {
  caseLine("CASE CompareLess int:${a} float:${b} -> ${Opaque(a) < Opaque(b)}")
  caseLine("CASE CompareLessEqual int:${a} float:${b} -> ${Opaque(a) <= Opaque(b)}")
  caseLine("CASE CompareGreater int:${a} float:${b} -> ${Opaque(a) > Opaque(b)}")
  caseLine("CASE CompareGreaterEqual int:${a} float:${b} -> ${Opaque(a) >= Opaque(b)}")
}
mod cmpSS(a: string, b: string) {
  caseLine("CASE CompareLess str:${a} str:${b} -> ${Opaque(a) < Opaque(b)}")
  caseLine("CASE CompareLessEqual str:${a} str:${b} -> ${Opaque(a) <= Opaque(b)}")
  caseLine("CASE CompareGreater str:${a} str:${b} -> ${Opaque(a) > Opaque(b)}")
  caseLine("CASE CompareGreaterEqual str:${a} str:${b} -> ${Opaque(a) >= Opaque(b)}")
}
mod cmpIS(a: int, b: string) {
  caseLine("CASE CompareLess int:${a} str:${b} -> ${Opaque(a) < Opaque(b)}")
  caseLine("CASE CompareLessEqual int:${a} str:${b} -> ${Opaque(a) <= Opaque(b)}")
  caseLine("CASE CompareGreater int:${a} str:${b} -> ${Opaque(a) > Opaque(b)}")
  caseLine("CASE CompareGreaterEqual int:${a} str:${b} -> ${Opaque(a) >= Opaque(b)}")
}
mod cmpBB(a: bool, b: bool) {
  caseLine("CASE CompareLess bool:${a} bool:${b} -> ${Opaque(a) < Opaque(b)}")
  caseLine("CASE CompareLessEqual bool:${a} bool:${b} -> ${Opaque(a) <= Opaque(b)}")
  caseLine("CASE CompareGreater bool:${a} bool:${b} -> ${Opaque(a) > Opaque(b)}")
  caseLine("CASE CompareGreaterEqual bool:${a} bool:${b} -> ${Opaque(a) >= Opaque(b)}")
}

mod runCompare() {
  caseLine("BEGIN compare 65")
  cmpII(0, 0) cmpII(1, 0) cmpII(0, 1) cmpII(-1, 0)
  cmpFF(0.0, 0.0) cmpFF(0.5, 1.0)
  cmpFFL("NaN", 0.0 / 0.0, "0", 0.0) cmpFFL("inf", 1.0 / 0.0, "1", 1.0)
  cmpIF(1, 1.0) cmpIF(1, 0.5)
  cmpSS("a", "b") cmpSS("", "a") cmpSS("10", "9")
  cmpIS(10, "9") cmpIS(1, "a")
  cmpBB(true, false)
  caseLine("CASE CompareLess int:1 unwired -> ${Opaque(1) < neverWired}")
  caseLine("END compare")
}

// -- Select family: pure `if-then-else` truthiness (Select gate) -------------
// Sentinel outputs 111 / 222 name which side was taken.
mod selI(c: int) {
  // Bind first: an inline ternary in interpolation-arg position constant-folds
  // away (Select AND Opaque vanish) — see task-3 review Critical.
  let r = if Opaque(c) then 111 else 222
  caseLine("CASE Select int:${c} - -> ${r}")
}
mod selF(c: float) {
  // Bind first: an inline ternary in interpolation-arg position constant-folds
  // away (Select AND Opaque vanish) — see task-3 review Critical.
  let r = if Opaque(c) then 111 else 222
  caseLine("CASE Select float:${c} - -> ${r}")
}
mod selFL(lc: string, c: float) {
  // Bind first: an inline ternary in interpolation-arg position constant-folds
  // away (Select AND Opaque vanish) — see task-3 review Critical.
  let r = if Opaque(c) then 111 else 222
  caseLine("CASE Select float:${lc} - -> ${r}")
}
mod selS(c: string) {
  // Bind first: an inline ternary in interpolation-arg position constant-folds
  // away (Select AND Opaque vanish) — see task-3 review Critical.
  let r = if Opaque(c) then 111 else 222
  caseLine("CASE Select str:${c} - -> ${r}")
}
mod selB(c: bool) {
  // Bind first: an inline ternary in interpolation-arg position constant-folds
  // away (Select AND Opaque vanish) — see task-3 review Critical.
  let r = if Opaque(c) then 111 else 222
  caseLine("CASE Select bool:${c} - -> ${r}")
}

mod runSelect() {
  caseLine("BEGIN select 15")
  selI(0) selI(1) selI(-1) selI(2)
  selF(0.0) selF(0.5) selFL("NaN", 0.0 / 0.0) selFL("inf", 1.0 / 0.0)
  selS("") selS("0") selS("a") selS("false")
  selB(true) selB(false)
  let rUnwired = if neverWired then 111 else 222
  caseLine("CASE Select unwired - -> ${rUnwired}")
  caseLine("END select")
}

// -- Branch family: exec-side `if` statement truthiness (Branch gate) --------
mod brI(c: int) {
  var got: string = ""
  if Opaque(c) { got = "A" } else { got = "B" }
  caseLine("CASE Branch int:${c} - -> ${got}")
}
mod brF(c: float) {
  var got: string = ""
  if Opaque(c) { got = "A" } else { got = "B" }
  caseLine("CASE Branch float:${c} - -> ${got}")
}
mod brFL(lc: string, c: float) {
  var got: string = ""
  if Opaque(c) { got = "A" } else { got = "B" }
  caseLine("CASE Branch float:${lc} - -> ${got}")
}
mod brS(c: string) {
  var got: string = ""
  if Opaque(c) { got = "A" } else { got = "B" }
  caseLine("CASE Branch str:${c} - -> ${got}")
}
mod brB(c: bool) {
  var got: string = ""
  if Opaque(c) { got = "A" } else { got = "B" }
  caseLine("CASE Branch bool:${c} - -> ${got}")
}

mod runBranch() {
  caseLine("BEGIN branch 15")
  brI(0) brI(1) brI(-1) brI(2)
  brF(0.0) brF(0.5) brFL("NaN", 0.0 / 0.0) brFL("inf", 1.0 / 0.0)
  brS("") brS("0") brS("a") brS("false")
  brB(true) brB(false)
  var gotU: string = ""
  if neverWired { gotU = "A" } else { gotU = "B" }
  caseLine("CASE Branch unwired - -> ${gotU}")
  caseLine("END branch")
}

// -- Math family: MathAdd/Subtract/Multiply/Divide/Modulo --------------------
mod mathII5(a: int, b: int) {
  caseLine("CASE MathAdd int:${a} int:${b} -> ${Opaque(a) + Opaque(b)}")
  caseLine("CASE MathSubtract int:${a} int:${b} -> ${Opaque(a) - Opaque(b)}")
  caseLine("CASE MathMultiply int:${a} int:${b} -> ${Opaque(a) * Opaque(b)}")
  caseLine("CASE MathDivide int:${a} int:${b} -> ${Opaque(a) / Opaque(b)}")
  caseLine("CASE MathModulo int:${a} int:${b} -> ${Opaque(a) % Opaque(b)}")
}
// Extra non-trivial-remainder observation.
mod mathIIModOnly(a: int, b: int) {
  caseLine("CASE MathModulo int:${a} int:${b} -> ${Opaque(a) % Opaque(b)}")
}
// Division/modulo-by-zero observation.
mod mathIIDivMod(a: int, b: int) {
  caseLine("CASE MathDivide int:${a} int:${b} -> ${Opaque(a) / Opaque(b)}")
  caseLine("CASE MathModulo int:${a} int:${b} -> ${Opaque(a) % Opaque(b)}")
}
mod mathFF5(a: float, b: float) {
  caseLine("CASE MathAdd float:${a} float:${b} -> ${Opaque(a) + Opaque(b)}")
  caseLine("CASE MathSubtract float:${a} float:${b} -> ${Opaque(a) - Opaque(b)}")
  caseLine("CASE MathMultiply float:${a} float:${b} -> ${Opaque(a) * Opaque(b)}")
  caseLine("CASE MathDivide float:${a} float:${b} -> ${Opaque(a) / Opaque(b)}")
  caseLine("CASE MathModulo float:${a} float:${b} -> ${Opaque(a) % Opaque(b)}")
}
mod mathFF5L(la: string, a: float, lb: string, b: float) {
  caseLine("CASE MathAdd float:${la} float:${lb} -> ${Opaque(a) + Opaque(b)}")
  caseLine("CASE MathSubtract float:${la} float:${lb} -> ${Opaque(a) - Opaque(b)}")
  caseLine("CASE MathMultiply float:${la} float:${lb} -> ${Opaque(a) * Opaque(b)}")
  caseLine("CASE MathDivide float:${la} float:${lb} -> ${Opaque(a) / Opaque(b)}")
  caseLine("CASE MathModulo float:${la} float:${lb} -> ${Opaque(a) % Opaque(b)}")
}
mod mathIF5(a: int, b: float) {
  caseLine("CASE MathAdd int:${a} float:${b} -> ${Opaque(a) + Opaque(b)}")
  caseLine("CASE MathSubtract int:${a} float:${b} -> ${Opaque(a) - Opaque(b)}")
  caseLine("CASE MathMultiply int:${a} float:${b} -> ${Opaque(a) * Opaque(b)}")
  caseLine("CASE MathDivide int:${a} float:${b} -> ${Opaque(a) / Opaque(b)}")
  caseLine("CASE MathModulo int:${a} float:${b} -> ${Opaque(a) % Opaque(b)}")
}
mod mathIB5(a: int, b: bool) {
  caseLine("CASE MathAdd int:${a} bool:${b} -> ${Opaque(a) + Opaque(b)}")
  caseLine("CASE MathSubtract int:${a} bool:${b} -> ${Opaque(a) - Opaque(b)}")
  caseLine("CASE MathMultiply int:${a} bool:${b} -> ${Opaque(a) * Opaque(b)}")
  caseLine("CASE MathDivide int:${a} bool:${b} -> ${Opaque(a) / Opaque(b)}")
  caseLine("CASE MathModulo int:${a} bool:${b} -> ${Opaque(a) % Opaque(b)}")
}
// String-concat-via-`+` observation: `+` is not a typed String,String
// operator rule (see catalog/operators.rs), but Opaque's wildcard type lets
// it resolve to the first matching rule (Float,Float) and lower to a real
// MathAdd gate carrying string-variant wires — an in-game-only observation
// of how the MathAdd gate handles non-numeric input.
mod mathSSAdd(a: string, b: string) {
  caseLine("CASE MathAdd str:${a} str:${b} -> ${Opaque(a) + Opaque(b)}")
}
mod mathSIAdd(a: string, b: int) {
  caseLine("CASE MathAdd str:${a} int:${b} -> ${Opaque(a) + Opaque(b)}")
}

mod runMath() {
  caseLine("BEGIN math 57")
  mathII5(1, 2) mathII5(0, 5) mathII5(-1, 1)
  mathIIModOnly(7, 3)
  mathIIDivMod(1, 0)
  mathFF5(0.5, 0.25) mathFF5(1.0, 0.0) mathFF5(0.0, 0.0) mathFF5L("NaN", 0.0 / 0.0, "1", 1.0)
  mathIF5(1, 0.5)
  mathIB5(1, true) mathIB5(1, false)
  mathSSAdd("a", "b") mathSSAdd("", "x")
  mathSIAdd("a", 1)
  caseLine("CASE MathAdd int:1 unwired -> ${Opaque(1) + neverWired}")
  caseLine("END math")
}

on grid {
  PrintToConsole("PROBE gate_semantics v${VERSION}")
  runEq()
  runBool()
  runCompare()
  runSelect()
  runBranch()
  runMath()
  PrintToConsole("PROBE done")
}
