/// Gate semantics probe — prints one CASE line per observed interaction.
/// Paste into a local world; it runs itself (ReadBrickGrid fires on paste).
/// Bump VERSION whenever the case matrix changes.
let VERSION = 3

let grid = ReadBrickGrid()

// A top-level `in` port that nothing ever wires. Reading it observes how a
// gate behaves when one of its inputs is left unconnected (its "unwired"
// resting value).
in neverWired: int

mod caseLine(msg: string) {
  PrintToConsole(msg)
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
  eqII(0, 0) eqII(1, 1) eqII(1, 0) eqII(-1, 1)
  eqIIL("9007199254740993", 9007199254740993, "9007199254740992", 9007199254740992)
  eqIS(1, "1") eqIS(0, "") eqIS(0, "0") eqIS(1, "a") eqIS(1, "1.0")
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

// ============================================================================
// v3 additions below. v2 chapters above are byte-identical to v2 — only new
// chapters are appended (render / strings / compositeMath / compositeOps /
// deferredOps). Composite (vector/rotator/color/quat) operands are ALWAYS
// described with a hardcoded label (never live-interpolated as an input —
// their render law is exactly what the `render` chapter is establishing),
// mirroring the eqFFL/mathFF5L "labeled variant" convention from v2. Scalar
// (int/float/bool/str) operands that render exactly are still interpolated
// live, matching v2.
// ============================================================================

// -- Render family: how each wire variant renders through FormatText -------
// Calibration probes, not gate-behavior probes: every other new chapter's
// composite results print through this same interpolation path, so these
// establish the rendering law they get interpreted against. Task 2 stores
// these in a dedicated `render` table section, not under `gates`.
mod renderI(label: string, v: int) {
  let x = Opaque(v)
  caseLine("CASE Render ${label} -> ${x}")
}
mod renderF(label: string, v: float) {
  let x = Opaque(v)
  caseLine("CASE Render ${label} -> ${x}")
}
mod renderB(label: string, v: bool) {
  let x = Opaque(v)
  caseLine("CASE Render ${label} -> ${x}")
}
mod renderS(label: string, v: string) {
  let x = Opaque(v)
  caseLine("CASE Render ${label} -> '${x}'")
}
mod renderVec(label: string, v: vector) {
  let x = Opaque(v)
  caseLine("CASE Render ${label} -> ${x}")
}
mod renderRot(label: string, v: rotator) {
  let x = Opaque(v)
  caseLine("CASE Render ${label} -> ${x}")
}
mod renderCol(label: string, v: color) {
  let x = Opaque(v)
  caseLine("CASE Render ${label} -> ${x}")
}
mod renderQuat(label: string, v: quat) {
  let x = Opaque(v)
  caseLine("CASE Render ${label} -> ${x}")
}

mod runRender() {
  caseLine("BEGIN render 32")
  renderI("int:0", 0) renderI("int:7", 7) renderI("int:-7", -7) renderI("int:999", 999)
  renderI("int:1000", 1000) renderI("int:9999", 9999) renderI("int:10000", 10000)
  renderI("int:999999", 999999) renderI("int:-1000000", -1000000)
  renderI("int:9007199254740993", 9007199254740993)
  renderF("float:1.0", 1.0) renderF("float:-1.0", -1.0) renderF("float:0.5", 0.5)
  renderF("float:1.0/3.0", 1.0 / 3.0) renderF("float:0.1+0.2", 0.1 + 0.2)
  renderF("float:2.0/3.0", 2.0 / 3.0) renderF("float:123456.789", 123456.789)
  renderF("float:1e-7", 1e-7) renderF("float:-0.0", -0.0)
  renderF("float:1e15", 1e15) renderF("float:1.5e-3", 1.5e-3)
  renderB("bool:true", true) renderB("bool:false", false)
  renderS("str:empty", "") renderS("str:a_b", "a b") renderS("str:multibyte", "π≈3")
  renderVec("vector:Vec(1.0,2.0,3.0)", Vec(1.0, 2.0, 3.0))
  renderVec("vector:Vec(0.5,-1.25,1.0/3.0)", Vec(Opaque(0.5), -1.25, 1.0 / 3.0))
  renderRot("rotator:Rotation(0.0,90.0,45.5)", Rotation(0.0, 90.0, 45.5))
  renderCol("color:Color(1.0,0.5,0.25)", Color(1.0, 0.5, 0.25))
  renderCol("color:Color(1.0,0.5,0.25,0.5)", Color(1.0, 0.5, 0.25, 0.5))
  renderQuat("quat:Quat(0.0,0.0,0.7071067811865476,0.7071067811865476)", Quat(0.0, 0.0, 0.7071067811865476, 0.7071067811865476))
  caseLine("END render")
}

// -- String family: Concatenate/FormatText/Length&friends/search/parse ------
// `..`'s String_Concatenate gate always lowers with Separator = "" (see
// lower/expr.rs and lower/mod.rs, both hardcode the empty-string property) —
// there is no source-level way to set it, so only the default-separator
// behavior is probed here, per the brief's fallback instruction.
mod concatSS(a: string, b: string) {
  caseLine("CASE String_Concatenate str:${a} str:${b} -> '${Opaque(a) .. Opaque(b)}'")
}
mod concatIS(a: int, b: string) {
  caseLine("CASE String_Concatenate int:${a} str:${b} -> '${Opaque(a) .. Opaque(b)}'")
}
mod concatSF(a: string, b: float) {
  caseLine("CASE String_Concatenate str:${a} float:${b} -> '${Opaque(a) .. Opaque(b)}'")
}
mod concatBS(a: bool, b: string) {
  caseLine("CASE String_Concatenate bool:${a} str:${b} -> '${Opaque(a) .. Opaque(b)}'")
}

// FormatText's `format` template argument is left a plain literal (never
// Opaque-wrapped): lower/call.rs's `literal_for_property_port` inlines any
// literal string argument directly as the gate's `FormatString` DATA
// property (the same representation `${...}` interpolation itself builds in
// lower/ops.rs::build_format_text) — Opaque would force it into a real wire
// on a port that every other call site in this compiler treats as
// config/data, not a value operand, so it stays literal like v2's own
// caseLine message strings. Only the substitution slots (a..g) are
// Opaque-armored operands under test.
mod fmtBase() {
  let r = Fmt("{0}", Opaque("hi"))
  caseLine("CASE String_FormatText tmpl:0str -> '${r}'")
}
mod fmtSurround() {
  let r = Fmt("a{0}b", Opaque("X"))
  caseLine("CASE String_FormatText tmpl:a0b -> '${r}'")
}
mod fmtTwoSlot() {
  let r = Fmt("{0}{1}", Opaque("A"), Opaque("B"))
  caseLine("CASE String_FormatText tmpl:01 -> '${r}'")
}
mod fmtReorder() {
  let r = Fmt("{1}-{0}", Opaque("A"), Opaque("B"))
  caseLine("CASE String_FormatText tmpl:10reorder -> '${r}'")
}
mod fmtVarInt() {
  let r = Fmt("{0}", Opaque(42))
  caseLine("CASE String_FormatText tmpl:0int -> '${r}'")
}
mod fmtVarFloat() {
  let r = Fmt("{0}", Opaque(0.5))
  caseLine("CASE String_FormatText tmpl:0float -> '${r}'")
}
mod fmtVarBool() {
  let r = Fmt("{0}", Opaque(true))
  caseLine("CASE String_FormatText tmpl:0bool -> '${r}'")
}
mod fmtVarStr() {
  let r = Fmt("{0}", Opaque("s"))
  caseLine("CASE String_FormatText tmpl:0str2 -> '${r}'")
}
mod fmtThreeSlot() {
  let r = Fmt("{0}-{1}-{2}", Opaque("A"), Opaque("B"), Opaque("C"))
  caseLine("CASE String_FormatText tmpl:3slot -> '${r}'")
}
mod fmtLiteralBrace() {
  let r = Fmt("literal{a}brace")
  caseLine("CASE String_FormatText tmpl:literalbrace -> '${r}'")
}
mod fmtUnwiredSlot() {
  let r = Fmt("{0}{1}", Opaque(1))
  caseLine("CASE String_FormatText tmpl:unwiredslot -> '${r}'")
}

mod strUnary4(s: string) {
  caseLine("CASE String_Length str:'${s}' -> ${Length(Opaque(s))}")
  caseLine("CASE String_ToLower str:'${s}' -> '${ToLower(Opaque(s))}'")
  caseLine("CASE String_ToUpper str:'${s}' -> '${ToUpper(Opaque(s))}'")
  caseLine("CASE String_Trim str:'${s}' -> '${Trim(Opaque(s))}'")
}
mod strContains(s: string, needle: string) {
  caseLine("CASE String_Contains str:'${s}' str:'${needle}' -> ${Contains(Opaque(s), Opaque(needle))}")
}
mod strStartsWith(s: string, pre: string) {
  caseLine("CASE String_StartsWith str:'${s}' str:'${pre}' -> ${StartsWith(Opaque(s), Opaque(pre))}")
}
mod strEndsWith(s: string, suf: string) {
  caseLine("CASE String_EndsWith str:'${s}' str:'${suf}' -> ${EndsWith(Opaque(s), Opaque(suf))}")
}
mod strSubstring(s: string, start: int, length: int) {
  caseLine("CASE String_Substring str:'${s}' int:${start} int:${length} -> '${Substring(Opaque(s), Opaque(start), Opaque(length))}'")
}
mod strFind(s: string, needle: string) {
  caseLine("CASE String_Find str:'${s}' str:'${needle}' -> ${Find(Opaque(s), Opaque(needle))}")
}
mod strReplace(s: string, search: string, repl: string) {
  caseLine("CASE String_Replace str:'${s}' str:'${search}' str:'${repl}' -> '${Replace(Opaque(s), Opaque(search), Opaque(repl))}'")
}
mod strParseInt(s: string) {
  caseLine("CASE String_ParseInt str:'${s}' -> ${ParseInt(Opaque(s))}")
}
mod strParseNumber(s: string) {
  caseLine("CASE String_ParseNumber str:'${s}' -> ${ParseNumber(Opaque(s))}")
}

mod runStrings() {
  caseLine("BEGIN strings 73")
  concatSS("a", "b") concatSS("", "x") concatSS("a", "")
  concatIS(7, "x") concatSF("x", 0.5) concatBS(true, "!")
  fmtBase() fmtSurround() fmtTwoSlot() fmtReorder()
  fmtVarInt() fmtVarFloat() fmtVarBool() fmtVarStr()
  fmtThreeSlot() fmtLiteralBrace() fmtUnwiredSlot()
  strUnary4("") strUnary4("a") strUnary4("AbC")
  strUnary4("  x  ") strUnary4("π≈3")
  strContains("hello world", "world")
  strContains("hello world", "xyz")
  strContains("hello world", "")
  strContains("Abc", "a")
  strStartsWith("hello world", "hello")
  strStartsWith("hello world", "xyz")
  strStartsWith("hello world", "")
  strStartsWith("Abc", "a")
  strEndsWith("hello world", "world")
  strEndsWith("hello world", "xyz")
  strEndsWith("hello world", "")
  strEndsWith("Abc", "C")
  strSubstring("hello", 1, 3)
  strSubstring("hello", 10, 3)
  strSubstring("hello", 1, 100)
  strSubstring("hello", -1, 3)
  strFind("hello world", "world")
  strFind("hello world", "xyz")
  strFind("hello world", "")
  strReplace("hello world", "world", "there")
  strReplace("hello world", "xyz", "there")
  strReplace("hello world", "", "X")
  strParseInt("42") strParseInt("-7") strParseInt("1.5")
  strParseInt("a") strParseInt("") strParseInt("9007199254740993")
  strParseInt(" 42 ")
  strParseNumber("42") strParseNumber("-7") strParseNumber("1.5")
  strParseNumber("a") strParseNumber("") strParseNumber("9007199254740993")
  strParseNumber(" 42 ")
  caseLine("END strings")
}

// -- CompositeMath family: MathAdd/Subtract/Multiply/Divide/Modulo on the -–
// -- vector variants of the SAME gates the `math` chapter already covers ---
// Operand components are halves/quarters (exact binary fractions) so every
// render is unambiguous. Vector operands are always hardcoded labels (see
// file banner) — only the live gate OUTPUT is interpolated.
mod mathVV5(la: string, a: vector, lb: string, b: vector) {
  caseLine("CASE MathAdd vector:${la} vector:${lb} -> ${Opaque(a) + Opaque(b)}")
  caseLine("CASE MathSubtract vector:${la} vector:${lb} -> ${Opaque(a) - Opaque(b)}")
  caseLine("CASE MathMultiply vector:${la} vector:${lb} -> ${Opaque(a) * Opaque(b)}")
  caseLine("CASE MathDivide vector:${la} vector:${lb} -> ${Opaque(a) / Opaque(b)}")
  caseLine("CASE MathModulo vector:${la} vector:${lb} -> ${Opaque(a) % Opaque(b)}")
}
mod mathVF5(la: string, a: vector, b: float) {
  caseLine("CASE MathAdd vector:${la} float:${b} -> ${Opaque(a) + Opaque(b)}")
  caseLine("CASE MathSubtract vector:${la} float:${b} -> ${Opaque(a) - Opaque(b)}")
  caseLine("CASE MathMultiply vector:${la} float:${b} -> ${Opaque(a) * Opaque(b)}")
  caseLine("CASE MathDivide vector:${la} float:${b} -> ${Opaque(a) / Opaque(b)}")
  caseLine("CASE MathModulo vector:${la} float:${b} -> ${Opaque(a) % Opaque(b)}")
}
mod mathFV5(a: float, lb: string, b: vector) {
  caseLine("CASE MathAdd float:${a} vector:${lb} -> ${Opaque(a) + Opaque(b)}")
  caseLine("CASE MathSubtract float:${a} vector:${lb} -> ${Opaque(a) - Opaque(b)}")
  caseLine("CASE MathMultiply float:${a} vector:${lb} -> ${Opaque(a) * Opaque(b)}")
  caseLine("CASE MathDivide float:${a} vector:${lb} -> ${Opaque(a) / Opaque(b)}")
  caseLine("CASE MathModulo float:${a} vector:${lb} -> ${Opaque(a) % Opaque(b)}")
}
mod mathVI5(la: string, a: vector, b: int) {
  caseLine("CASE MathAdd vector:${la} int:${b} -> ${Opaque(a) + Opaque(b)}")
  caseLine("CASE MathSubtract vector:${la} int:${b} -> ${Opaque(a) - Opaque(b)}")
  caseLine("CASE MathMultiply vector:${la} int:${b} -> ${Opaque(a) * Opaque(b)}")
  caseLine("CASE MathDivide vector:${la} int:${b} -> ${Opaque(a) / Opaque(b)}")
  caseLine("CASE MathModulo vector:${la} int:${b} -> ${Opaque(a) % Opaque(b)}")
}
mod mathVecNaNAdd() {
  let a = Opaque(Vec(0.0 / 0.0, 1.0, 2.0))
  let b = Opaque(Vec(1.0, 1.0, 1.0))
  let r = a + b
  caseLine("CASE MathAdd vector:Vec(NaN,1,2) vector:Vec(1,1,1) -> ${r}")
}
// Identity-carrier candidate #1 (the other, VecScale(v,1.0), is in
// compositeOps) — Task 5 needs one of the two certified as exact identity.
mod mathVecIdentityAdd() {
  let a = Opaque(Vec(0.5, 0.25, -0.75))
  let b = Opaque(Vec(0.0, 0.0, 0.0))
  let r = a + b
  caseLine("CASE MathAdd vector:Vec(0.5,0.25,-0.75) vector:Vec(0,0,0) -> ${r}")
}

mod runCompositeMath() {
  caseLine("BEGIN compositeMath 22")
  mathVV5("Vec(0.5,0.25,-0.75)", Vec(0.5, 0.25, -0.75), "Vec(0.25,0.5,0.75)", Vec(0.25, 0.5, 0.75))
  mathVF5("Vec(0.5,0.25,-0.75)", Vec(0.5, 0.25, -0.75), 0.5)
  mathFV5(0.5, "Vec(0.5,0.25,-0.75)", Vec(0.5, 0.25, -0.75))
  mathVI5("Vec(0.5,0.25,-0.75)", Vec(0.5, 0.25, -0.75), 2)
  mathVecNaNAdd()
  mathVecIdentityAdd()
  caseLine("END compositeMath")
}

// -- CompositeOps family: constructors/decomposers/vector-rotation gates ---
mod compositeMakeCases() {
  let x = Opaque(1.5)
  let y = Opaque(-2.5)
  let z = Opaque(0.75)
  let vecR = Vec(x, y, z)
  caseLine("CASE MakeVector float:1.5 float:-2.5 float:0.75 -> ${vecR}")
  let pitch = Opaque(30.0)
  let yaw = Opaque(60.0)
  let roll = Opaque(90.0)
  let rotR = Rotation(pitch, yaw, roll)
  caseLine("CASE MakeRotation float:30.0 float:60.0 float:90.0 -> ${rotR}")
  let qx = Opaque(0.0)
  let qy = Opaque(0.0)
  let qz = Opaque(0.7071067811865476)
  let qw = Opaque(0.7071067811865476)
  let quatR = Quat(qx, qy, qz, qw)
  caseLine("CASE MakeQuaternion float:0.0 float:0.0 float:0.7071067811865476 float:0.7071067811865476 -> ${quatR}")
  let cr = Opaque(1.0)
  let cg = Opaque(0.5)
  let cb = Opaque(0.25)
  let ca = Opaque(0.75)
  let colR = Color(cr, cg, cb, ca)
  caseLine("CASE MakeColor float:1.0 float:0.5 float:0.25 float:0.75 -> ${colR}")
  let sr = Opaque(255)
  let sg = Opaque(128)
  let sb = Opaque(0)
  let sa = Opaque(255)
  let srgbR = ColorSRGB(sr, sg, sb, sa)
  caseLine("CASE MakeColorSRGB int:255 int:128 int:0 int:255 -> ${srgbR}")
  let hex = Opaque("#ff8800")
  let hexR = ColorHex(hex)
  caseLine("CASE MakeColorHex str:#ff8800 -> ${hexR}")
  let cForHex = Opaque(Color(1.0, 0.5, 0.0))
  let hexOut = ToHex(cForHex)
  caseLine("CASE ColorToHex color:Color(1.0,0.5,0.0) -> ${hexOut}")
}
mod vecScaleCases() {
  let v = Opaque(Vec(0.5, 0.25, -0.75))
  let one = Opaque(1.0)
  let two = Opaque(2.0)
  let rIdentity = ScaleVec(v, one)
  let rDouble = ScaleVec(v, two)
  caseLine("CASE VecScale vector:Vec(0.5,0.25,-0.75) float:1.0 -> ${rIdentity}")
  caseLine("CASE VecScale vector:Vec(0.5,0.25,-0.75) float:2.0 -> ${rDouble}")
}
mod compositeVecOpsCases() {
  let v1 = Opaque(Vec(0.5, 0.25, -0.75))
  let v2 = Opaque(Vec(0.25, 0.5, 0.75))
  let dot = Dot(v1, v2)
  caseLine("CASE VecDotProduct vector:Vec(0.5,0.25,-0.75) vector:Vec(0.25,0.5,0.75) -> ${dot}")
  let cross = Cross(v1, v2)
  caseLine("CASE VecCrossProduct vector:Vec(0.5,0.25,-0.75) vector:Vec(0.25,0.5,0.75) -> ${cross}")
  let magSq = MagnitudeSq(v1)
  caseLine("CASE VecMagnitudeSquared vector:Vec(0.5,0.25,-0.75) -> ${magSq}")
  let distSq = DistanceSq(v1, v2)
  caseLine("CASE VecDistanceSquared vector:Vec(0.5,0.25,-0.75) vector:Vec(0.25,0.5,0.75) -> ${distSq}")
}
// 90°-multiple rotations (about Z; components sin/cos of the half-angle,
// same fixed literals every run) plus ONE 45° case per the brief.
mod rotateVectorCases() {
  let q90 = Opaque(Quat(0.0, 0.0, 0.7071067811865476, 0.7071067811865476))
  let q180 = Opaque(Quat(0.0, 0.0, 1.0, 0.0))
  let q45 = Opaque(Quat(0.0, 0.0, 0.3826834323650898, 0.9238795325112867))
  let v1 = Opaque(Vec(1.0, 0.0, 0.0))
  let v2 = Opaque(Vec(0.0, 1.0, 0.0))
  let r90 = Rotate(v1, q90)
  let r180 = Rotate(v2, q180)
  let r45 = Rotate(v1, q45)
  caseLine("CASE RotateVector vector:Vec(1,0,0) quat:Quat(0.0,0.0,0.7071067811865476,0.7071067811865476) -> ${r90}")
  caseLine("CASE RotateVector vector:Vec(0,1,0) quat:Quat(0.0,0.0,1.0,0.0) -> ${r180}")
  caseLine("CASE RotateVector vector:Vec(1,0,0) quat:Quat(0.0,0.0,0.3826834323650898,0.9238795325112867) -> ${r45}")
}
mod compositeQuatOpsCases() {
  let q90 = Opaque(Quat(0.0, 0.0, 0.7071067811865476, 0.7071067811865476))
  let q180 = Opaque(Quat(0.0, 0.0, 1.0, 0.0))
  let inv = Invert(q90)
  caseLine("CASE InvertRotation quat:Quat(0.0,0.0,0.7071067811865476,0.7071067811865476) -> ${inv}")
  let qdot = QuatDot(q90, q180)
  caseLine("CASE QuatDotProduct quat:Quat(0.0,0.0,0.7071067811865476,0.7071067811865476) quat:Quat(0.0,0.0,1.0,0.0) -> ${qdot}")
}
mod cmpEqVecL(la: string, a: vector, lb: string, b: vector) {
  caseLine("CASE CompareEqual vector:${la} vector:${lb} -> ${Opaque(a) == Opaque(b)}")
  caseLine("CASE CompareNotEqual vector:${la} vector:${lb} -> ${Opaque(a) != Opaque(b)}")
}
mod cmpEqColL(la: string, a: color, lb: string, b: color) {
  caseLine("CASE CompareEqual color:${la} color:${lb} -> ${Opaque(a) == Opaque(b)}")
  caseLine("CASE CompareNotEqual color:${la} color:${lb} -> ${Opaque(a) != Opaque(b)}")
}
mod cmpEqRotL(la: string, a: rotator, lb: string, b: rotator) {
  caseLine("CASE CompareEqual rotator:${la} rotator:${lb} -> ${Opaque(a) == Opaque(b)}")
  caseLine("CASE CompareNotEqual rotator:${la} rotator:${lb} -> ${Opaque(a) != Opaque(b)}")
}
mod cmpEqQuatL(la: string, a: quat, lb: string, b: quat) {
  caseLine("CASE CompareEqual quat:${la} quat:${lb} -> ${Opaque(a) == Opaque(b)}")
  caseLine("CASE CompareNotEqual quat:${la} quat:${lb} -> ${Opaque(a) != Opaque(b)}")
}

mod runCompositeOps() {
  caseLine("BEGIN compositeOps 34")
  compositeMakeCases()
  vecScaleCases()
  compositeVecOpsCases()
  rotateVectorCases()
  compositeQuatOpsCases()
  cmpEqVecL("Vec(0.5,0.25,-0.75)", Vec(0.5, 0.25, -0.75), "Vec(0.5,0.25,-0.75)", Vec(0.5, 0.25, -0.75))
  cmpEqVecL("Vec(0.5,0.25,-0.75)", Vec(0.5, 0.25, -0.75), "Vec(0.25,0.5,0.75)", Vec(0.25, 0.5, 0.75))
  cmpEqColL("Color(1.0,0.5,0.25)", Color(1.0, 0.5, 0.25), "Color(1.0,0.5,0.25)", Color(1.0, 0.5, 0.25))
  cmpEqColL("Color(1.0,0.5,0.25)", Color(1.0, 0.5, 0.25), "Color(1.0,0.5,0.5)", Color(1.0, 0.5, 0.5))
  cmpEqRotL("Rotation(0.0,90.0,0.0)", Rotation(0.0, 90.0, 0.0), "Rotation(0.0,90.0,0.0)", Rotation(0.0, 90.0, 0.0))
  cmpEqRotL("Rotation(0.0,90.0,0.0)", Rotation(0.0, 90.0, 0.0), "Rotation(0.0,180.0,0.0)", Rotation(0.0, 180.0, 0.0))
  cmpEqQuatL("Quat(0.0,0.0,0.7071067811865476,0.7071067811865476)", Quat(0.0, 0.0, 0.7071067811865476, 0.7071067811865476), "Quat(0.0,0.0,0.7071067811865476,0.7071067811865476)", Quat(0.0, 0.0, 0.7071067811865476, 0.7071067811865476))
  cmpEqQuatL("Quat(0.0,0.0,0.7071067811865476,0.7071067811865476)", Quat(0.0, 0.0, 0.7071067811865476, 0.7071067811865476), "Quat(0.0,0.0,1.0,0.0)", Quat(0.0, 0.0, 1.0, 0.0))
  caseLine("END compositeOps")
}

// -- DeferredOps family: collected only — never folded (see eval table) ----
// One case per reachable deferred gate. `ColorConvert` is DROPPED: it has no
// source-level builtin (crates/wirescript/src/catalog/calls.rs) and no
// gate_class constant at all in this build (crates/wirescript/src/ir/gate_class.rs)
// — unreachable from source, not merely uncalled.
mod runDeferredOps() {
  caseLine("BEGIN deferredOps 10")
  let v1 = Opaque(Vec(0.5, 0.25, -0.75))
  let v2 = Opaque(Vec(0.25, 0.5, 0.75))
  let mag = Magnitude(v1)
  caseLine("CASE VecMagnitude vector:Vec(0.5,0.25,-0.75) -> ${mag}")
  let norm = Normalize(v1)
  caseLine("CASE VecNormalize vector:Vec(0.5,0.25,-0.75) -> ${norm}")
  let dist = Distance(v1, v2)
  caseLine("CASE VecDistance vector:Vec(0.5,0.25,-0.75) vector:Vec(0.25,0.5,0.75) -> ${dist}")
  let q1 = Opaque(Quat(0.0, 0.0, 0.7071067811865476, 0.7071067811865476))
  let q2 = Opaque(Quat(0.0, 0.0, 1.0, 0.0))
  let alpha = Opaque(0.5)
  let slerp = Slerp(q1, q2, alpha)
  caseLine("CASE QuatSlerp quat:Quat(0.0,0.0,0.7071067811865476,0.7071067811865476) quat:Quat(0.0,0.0,1.0,0.0) float:0.5 -> ${slerp}")
  let axis = Opaque(Vec(0.0, 0.0, 1.0))
  let angle = Opaque(1.5707963267948966)
  let fromAxis = RotationByAngle(axis, angle)
  caseLine("CASE QuatFromAxisAngle vector:Vec(0.0,0.0,1.0) float:1.5707963267948966 -> ${fromAxis}")
  let angleBetween = AngleTo(q1, q2)
  caseLine("CASE QuatAngleBetween quat:Quat(0.0,0.0,0.7071067811865476,0.7071067811865476) quat:Quat(0.0,0.0,1.0,0.0) -> ${angleBetween}")
  let between = RotationTo(v1, v2)
  caseLine("CASE QuatBetween vector:Vec(0.5,0.25,-0.75) vector:Vec(0.25,0.5,0.75) -> ${between}")
  let dirToRot = ToRotation(v1)
  caseLine("CASE DirectionToRotation vector:Vec(0.5,0.25,-0.75) -> ${dirToRot}")
  let rotToDir = ToDirection(q1)
  caseLine("CASE RotationToDirection quat:Quat(0.0,0.0,0.7071067811865476,0.7071067811865476) -> ${rotToDir}")
  let c1 = Opaque(Color(1.0, 0.5, 0.25))
  let c2 = Opaque(Color(0.0, 0.5, 1.0))
  let calpha = Opaque(0.5)
  let blend = ColorBlend(c1, c2, calpha)
  caseLine("CASE ColorBlend color:Color(1.0,0.5,0.25) color:Color(0.0,0.5,1.0) float:0.5 -> ${blend}")
  caseLine("END deferredOps")
}

on grid {
  PrintToConsole("PROBE gate_semantics v${VERSION}")
  runEq()
  runBool()
  runCompare()
  runSelect()
  runBranch()
  runMath()
  runRender()
  runStrings()
  runCompositeMath()
  runCompositeOps()
  runDeferredOps()
  PrintToConsole("PROBE done")
}
