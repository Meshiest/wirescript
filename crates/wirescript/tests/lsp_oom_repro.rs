//! Repro harness for the 2026-07-01 LSP memory blowup (25+ GB).
//!
//! The workspace state at the time: projects/input/lib.ws contained mod
//! signatures with `-> (...)` on a continuation line (a parse error — the
//! param list doesn't eat newlines, so the arrow is never seen), and
//! cursor.ws (open in the editor) imported it. The LSP analyze path is
//! parse → resolve → typecheck → collect_estimates; the CLI checker stops
//! after typecheck and terminated fine, so the suspect is the estimates /
//! template-compilation stage on the malformed resolved AST.
//!
//! Each test arms a watchdog that hard-exits the process if the stage runs
//! away, so a regression can't take the machine down with it.

use wirescript::collections::HashMap;

use wirescript::analysis::collect_estimates;
use wirescript::resolve::{resolve, MemLoader};
use wirescript::typecheck::typecheck;

/// Hard-exit the test process if it's still alive after `secs`. Keeps a
/// runaway loop from eating the machine (this repro previously hit 25+ GB).
fn arm_watchdog(secs: u64, label: &'static str) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(secs));
        eprintln!("WATCHDOG: {label} still running after {secs}s — aborting process");
        std::process::exit(86);
    });
}

/// The original broken lib.ws, verbatim shape: `-> (...)` return lists on
/// their own continuation line after the parameter list's closing paren.
const BROKEN_LIB: &str = r#"
let LINE_HEIGHT_PER_WORLD = 1.0
let KERNING_PER_WORLD = 1.0
let CHAR_ASPECT = 0.5
let ANCHOR_TL_X = 0.0
let ANCHOR_TL_Y = 0.0
let GLYPH_SCALE_X = 1.0
let GLYPH_SCALE_Y = -1.0
let GLYPH_BASE_X = 0.0
let GLYPH_BASE_Y = 0.0
let GLYPH_OFFSET_Z = 0.1
let MAX_DIST = 10000.0
let PLANE_EPS = 2.0
let MIN_SIZE = 1.0
let GRID_MIN = 1
let GRID_MAX = 128
let UNITS_NORMALIZED = 0
let UNITS_CELLS = 1
let UNITS_WORLD = 2

mod DisplayBasis(tl: vector, tr: vector, bl: vector, br: vector)
    -> (uHat: vector, vHat: vector, width: float, height: float) {
  let u = ((tr - tl) + (br - bl)) * 0.5
  let v = ((bl - tl) + (br - tr)) * 0.5
  let w = Magnitude(u)
  let h = Magnitude(v)
  out width = w
  out height = h
  out uHat = if w > 0.001 then u / w else Vec(1.0, 0.0, 0.0)
  out vHat = if h > 0.001 then v / h else Vec(0.0, 0.0, -1.0)
}

mod ProjectPoint(hit: vector, origin: vector, uHat: vector, vHat: vector)
    -> (px: float, py: float, planeDist: float) {
  let d = hit - origin
  out px = Dot(d, uHat)
  out py = Dot(d, vHat)
  out planeDist = Dot(d, Cross(uHat, vHat))
}

mod ToUnits(px: float, py: float, width: float, height: float,
            cols: float, rows: float, unitMode: int) -> (x: float, y: float) {
  out x = if unitMode == UNITS_CELLS then clamp(floor(px / width * cols), 0.0, cols - 1.0)
          else if unitMode == UNITS_WORLD then px
          else px / width
  out y = if unitMode == UNITS_CELLS then clamp(floor(py / height * rows), 0.0, rows - 1.0)
          else if unitMode == UNITS_WORLD then py
          else py / height
}

mod TextFit(width: float, height: float, cols: float, rows: float)
    -> (lineHeight: float, kerning: float) {
  let cellH = height / rows
  let cellW = width / cols
  out lineHeight = cellH * LINE_HEIGHT_PER_WORLD
  out kerning = (cellW - CHAR_ASPECT * cellH) * KERNING_PER_WORLD
}

mod InBounds(px: float, py: float, planeDist: float,
             width: float, height: float) -> bool {
  return px >= 0.0 && px <= width && py >= 0.0 && py <= height
      && abs(planeDist) < PLANE_EPS
}
"#;

/// cursor.ws as it was: imports the broken lib, calls the broken mods.
const CURSOR_WS: &str = r#"
import { ProjectPoint, ToUnits, InBounds, MAX_DIST, ANCHOR_TL_X, ANCHOR_TL_Y, GLYPH_SCALE_X, GLYPH_SCALE_Y, GLYPH_BASE_X, GLYPH_BASE_Y, GLYPH_OFFSET_Z } from "lib"

in player: character
in occupied: bool
in origin: vector
in uAxis: vector
in vAxis: vector
in width: float
in height: float
in cols: int
in rows: int
in unitMode: int
in calibrated: bool

var lastHit: vector = Vec(0.0, 0.0, 0.0)
var hitOk: bool = false

let running = occupied && calibrated
buffer tick: int = tick + (if running then 1 else 0)

on if running then tick else 0 {
  let aim = player.GetAim()
  let r = Sweep(aim.Origin, aim.Direction, MAX_DIST, ignore = player,
                detectBricks = true, detectMap = true)
  on r.Hit {
    lastHit = r.HitLocation
    hitOk = true
  }
  on r.Miss {
    hitOk = false
  }
}

on !running {
  hitOk = false
}

let p = ProjectPoint(lastHit.Value, origin, uAxis, vAxis)
let units = ToUnits(p.px, p.py, width, height, cols, rows, unitMode)
let showing = hitOk.Value && InBounds(p.px, p.py, p.planeDist, width, height)

out cursorX: float = units.x
out cursorY: float = units.y
out onDisplay: bool = showing
out glyphText: string = "+"
out glyphAnchorX: float = ANCHOR_TL_X
out glyphAnchorY: float = ANCHOR_TL_Y
out glyphOffsetX: float = p.px * GLYPH_SCALE_X + GLYPH_BASE_X
out glyphOffsetY: float = p.py * GLYPH_SCALE_Y + GLYPH_BASE_Y
out glyphOffsetZ: float = GLYPH_OFFSET_Z
out glyphEnabled: bool = showing
"#;

/// cursor.ws EXACTLY as originally written: multi-line import braces with a
/// trailing comma (no corpus precedent for this shape).
const ORIG_CURSOR_WS: &str = r#"
import {
  ProjectPoint, ToUnits, InBounds,
  MAX_DIST, ANCHOR_TL_X, ANCHOR_TL_Y,
  GLYPH_SCALE_X, GLYPH_SCALE_Y, GLYPH_BASE_X, GLYPH_BASE_Y, GLYPH_OFFSET_Z,
} from "lib"

in player: character
in occupied: bool
in origin: vector
in uAxis: vector
in vAxis: vector
in width: float
in height: float
in cols: int
in rows: int
in unitMode: int
in calibrated: bool

var lastHit: vector = Vec(0.0, 0.0, 0.0)
var hitOk: bool = false

let running = occupied && calibrated
buffer tick: int = tick + (if running then 1 else 0)

on if running then tick else 0 {
  let aim = player.GetAim()
  let r = Sweep(aim.Origin, aim.Direction, MAX_DIST, ignore = player,
                detectBricks = true, detectMap = true)
  on r.Hit {
    lastHit = r.HitLocation
    hitOk = true
  }
  on r.Miss {
    hitOk = false
  }
}

on !running {
  hitOk = false
}

let p = ProjectPoint(lastHit.Value, origin, uAxis, vAxis)
let units = ToUnits(p.px, p.py, width, height, cols, rows, unitMode)
let showing = hitOk.Value && InBounds(p.px, p.py, p.planeDist, width, height)

out cursorX: float = units.x
out cursorY: float = units.y
out onDisplay: bool = showing
out glyphText: string = "+"
out glyphAnchorX: float = ANCHOR_TL_X
out glyphAnchorY: float = ANCHOR_TL_Y
out glyphOffsetX: float = p.px * GLYPH_SCALE_X + GLYPH_BASE_X
out glyphOffsetY: float = p.py * GLYPH_SCALE_Y + GLYPH_BASE_Y
out glyphOffsetZ: float = GLYPH_OFFSET_Z
out glyphEnabled: bool = showing
"#;

/// calibrate.ws EXACTLY as originally written: multi-line import + UNWRAPPED
/// multi-line if-then-else chains at top level (no parens).
const ORIG_CALIBRATE_WS: &str = r#"
import {
  DisplayBasis, TextFit,
  ANCHOR_TL_X, ANCHOR_TL_Y, MAX_DIST, MIN_SIZE, GRID_MIN, GRID_MAX,
  UNITS_NORMALIZED, UNITS_CELLS,
} from "lib"

in player: character
in start: exec
in clicked: bool
in userText: string

var state: int = 0
var missFlag: bool = false

var cTL: vector = Vec(0.0, 0.0, 0.0)
var cTR: vector = Vec(0.0, 0.0, 0.0)
var cBL: vector = Vec(0.0, 0.0, 0.0)
var cBR: vector = Vec(0.0, 0.0, 0.0)

var colsW: int = 32
var rowsW: int = 16
var unitW: int = 0

var originL: vector = Vec(0.0, 0.0, 0.0)
var uAxisL: vector = Vec(1.0, 0.0, 0.0)
var vAxisL: vector = Vec(0.0, 0.0, -1.0)
var widthL: float = 100.0
var heightL: float = 100.0
var colsL: int = 32
var rowsL: int = 16
var unitL: int = 0
var lineHeightL: float = 10.0
var kerningL: float = 0.0
var calibratedL: bool = false

let inp = player.InputReader()
let confirmPress = inp.Jump || clicked
let upPress = inp.Forward > 0.5
let downPress = inp.Forward < -0.5
let rightPress = inp.Right > 0.5
let leftPress = inp.Right < -0.5

let basis = DisplayBasis(cTL.Value, cTR.Value, cBL.Value, cBR.Value)
let liveFit = TextFit(basis.width, basis.height, colsW.Value, rowsW.Value)

on start {
  state = 1
  missFlag = false
}

let capture: exec

on confirmPress {
  if state >= 1 && state <= 4 {
    emit capture
  } else if state == 5 {
    state = 6
  } else if state == 6 {
    if basis.width > MIN_SIZE && basis.height > MIN_SIZE {
      originL = cTL
      uAxisL = basis.uHat
      vAxisL = basis.vHat
      widthL = basis.width
      heightL = basis.height
      colsL = colsW
      rowsL = rowsW
      unitL = unitW
      lineHeightL = liveFit.lineHeight
      kerningL = liveFit.kerning
      calibratedL = true
      state = 0
    } else {
      missFlag = true
      state = 1
    }
  }
}

on capture {
  let aim = player.GetAim()
  let r = Sweep(aim.Origin, aim.Direction, MAX_DIST, ignore = player,
                detectBricks = true, detectMap = true)
  on r.Hit {
    missFlag = false
    if state == 1 { cTL = r.HitLocation }
    if state == 2 { cTR = r.HitLocation }
    if state == 3 { cBL = r.HitLocation }
    if state == 4 { cBR = r.HitLocation }
    state = state + 1
  }
  on r.Miss {
    missFlag = true
  }
}

on upPress {
  if state == 5 { rowsW = min(rowsW + 1, GRID_MAX) }
  else if state == 6 { unitW = (unitW + 1) % 3 }
}
on downPress {
  if state == 5 { rowsW = max(rowsW - 1, GRID_MIN) }
  else if state == 6 { unitW = (unitW + 2) % 3 }
}
on rightPress {
  if state == 5 { colsW = min(colsW + 1, GRID_MAX) }
}
on leftPress {
  if state == 5 { colsW = max(colsW - 1, GRID_MIN) }
}

let st = state.Value
let missLine = if missFlag.Value then "\nNO HIT - aim at the display surface" else ""
let unitName = if unitW.Value == UNITS_NORMALIZED then "normalized 0..1"
               else if unitW.Value == UNITS_CELLS then "character cells"
               else "world units"
let instrText =
  if st == 1 then "CALIBRATE 1/6\nAim at the TOP-LEFT corner\nSPACE to capture${missLine}"
  else if st == 2 then "CALIBRATE 2/6\nAim at the TOP-RIGHT corner\nSPACE to capture${missLine}"
  else if st == 3 then "CALIBRATE 3/6\nAim at the BOTTOM-LEFT corner\nSPACE to capture${missLine}"
  else if st == 4 then "CALIBRATE 4/6\nAim at the BOTTOM-RIGHT corner\nSPACE to capture${missLine}"
  else if st == 5 then "CALIBRATE 5/6\nGrid: ${colsW.Value} cols x ${rowsW.Value} rows\nA/D = cols, W/S = rows\nSPACE to confirm"
  else "CALIBRATE 6/6\nCursor units: ${unitName}\nW/S = change\nSPACE to finish"

buffer hudTick: int = hudTick + (if state.Value > 0 then 1 else 0)
on if state.Value > 0 && hudTick % 15 == 0 then hudTick else 0 {
  player.DisplayText(instrText, textId = 90, lifetime = 1.0)
}

out text: string =
  if st > 0 then instrText
  else if calibratedL.Value then userText
  else "[cursor] pulse 'start' to calibrate"

let adjusting = st == 5 || st == 6
out lineHeight: float = if adjusting then liveFit.lineHeight else lineHeightL.Value
out kerning: float = if adjusting then liveFit.kerning else kerningL.Value

out anchorX: float = ANCHOR_TL_X
out anchorY: float = ANCHOR_TL_Y
out offsetX: float = 0.0
out offsetY: float = 0.0
out offsetZ: float = 0.0
out font = $BrickFontDescriptor/IosevkaTerm

out origin: vector = originL.Value
out uAxis: vector = uAxisL.Value
out vAxis: vector = vAxisL.Value
out width: float = widthL.Value
out height: float = heightL.Value
out cols: int = colsL.Value
out rows: int = rowsL.Value
out unitMode: int = unitL.Value
out calibrated: bool = calibratedL.Value
"#;

fn mem_loader() -> MemLoader {
    let mut files = HashMap::default();
    files.insert("lib.ws".to_string(), BROKEN_LIB.to_string());
    files.insert("cursor.ws".to_string(), CURSOR_WS.to_string());
    MemLoader { files }
}

fn full_mem_loader() -> MemLoader {
    let mut files = HashMap::default();
    files.insert("lib.ws".to_string(), BROKEN_LIB.to_string());
    files.insert("cursor.ws".to_string(), ORIG_CURSOR_WS.to_string());
    files.insert("calibrate.ws".to_string(), ORIG_CALIBRATE_WS.to_string());
    MemLoader { files }
}

/// One document's full LSP surface: everything Backend::analyze computes,
/// plus the request handlers VS Code fires automatically (inlay hints) and
/// on demand (formatting, hover).
fn lsp_surface(label: &str, source: &str, file: &str, loader: &MemLoader, hover_sweep: bool) {
    use wirescript::analysis::{
        collect_inlay_hints, collect_symbols_for_file, format_wirescript, hover_at,
    };

    let t = std::time::Instant::now();
    let pre = wirescript::parse(source, file);
    let resolved = resolve(source, file, loader);
    let tc = typecheck(&resolved.ast, file);
    eprintln!("[{label}] parse+resolve+typecheck: {:?}", t.elapsed());

    let t = std::time::Instant::now();
    let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some(file));
    eprintln!("[{label}] symbols: {:?} ({})", t.elapsed(), symbols.len());

    let t = std::time::Instant::now();
    let estimates = collect_estimates(&resolved.ast, &tc, file);
    eprintln!("[{label}] estimates: {:?} ({})", t.elapsed(), estimates.len());

    let t = std::time::Instant::now();
    let hints = collect_inlay_hints(source, &pre.ast, &tc.type_of_expr, file);
    eprintln!("[{label}] inlay hints: {:?} ({})", t.elapsed(), hints.len());

    let t = std::time::Instant::now();
    let formatted = format_wirescript(source, "  ");
    eprintln!("[{label}] format: {:?} ({} bytes)", t.elapsed(), formatted.len());

    if hover_sweep {
        let t = std::time::Instant::now();
        let mut hovers = 0usize;
        for (line_no, line) in source.lines().enumerate() {
            for col in (0..line.len()).step_by(4) {
                if hover_at(
                    source, file, &symbols, &tc.type_of_expr, &resolved.doc_comments,
                    &tc.if_contexts, &tc.var_read_contexts, &estimates, line_no, col,
                )
                .is_some()
                {
                    hovers += 1;
                }
            }
        }
        eprintln!("[{label}] hover sweep: {:?} ({hovers} hits)", t.elapsed());
    }
}

/// Regression for the 25 GB LSP blowup: a newline anywhere inside
/// `import { ... }` braces used to spin parse_import_decl's binding loop
/// forever (expect(Ident)/expect(Comma) both fail on Newline without
/// consuming it), pushing an ImportBinding + two diagnostics per iteration.
#[test]
fn multiline_named_imports_parse() {
    arm_watchdog(15, "multiline_named_imports_parse");
    use wirescript::ast::{ImportKind, TopDecl};

    let variants: Vec<(&str, &str, usize)> = vec![
        ("single-line", "import { A } from \"lib\"\n", 1),
        ("newline-mid-list", "import { A,\n B } from \"lib\"\n", 2),
        ("newline-after-brace", "import {\n A } from \"lib\"\n", 1),
        ("trailing-comma", "import { A, } from \"lib\"\n", 1),
        ("trailing-comma-newline", "import { A,\n} from \"lib\"\n", 1),
        ("newline-before-brace-no-comma", "import { A\n} from \"lib\"\n", 1),
        (
            "full-multiline-with-alias",
            "import {\n  A,\n  B as C,\n  D,\n} from \"lib\"\n",
            3,
        ),
    ];
    for (name, src, expect_bindings) in variants {
        let t = std::time::Instant::now();
        let parsed = wirescript::parse(src, "t.ws");
        assert!(
            parsed.diagnostics.is_empty(),
            "{name}: expected clean parse, got {:?}",
            parsed.diagnostics
        );
        let TopDecl::Import(imp) = &parsed.ast.decls[0] else {
            panic!("{name}: first decl is not an import");
        };
        let ImportKind::Named(bindings) = &imp.kind else {
            panic!("{name}: not a named import");
        };
        assert_eq!(bindings.len(), expect_bindings, "{name}: binding count");
        eprintln!("{name}: OK in {:?} ({} bindings)", t.elapsed(), bindings.len());
    }
}

/// Garbage tokens inside import braces must terminate with diagnostics, not
/// stall the loop (both expects fail without consuming).
#[test]
fn garbage_in_import_braces_terminates() {
    arm_watchdog(15, "garbage_in_import_braces_terminates");
    for src in [
        "import { 5 5 } from \"lib\"\n",
        "import { , , } from \"lib\"\n",
        "import { A 5 B } from \"lib\"\n",
        "import { \"x\" } from \"lib\"\n",
    ] {
        let t = std::time::Instant::now();
        let parsed = wirescript::parse(src, "t.ws");
        assert!(!parsed.diagnostics.is_empty(), "garbage should diagnose: {src:?}");
        eprintln!("{src:?}: OK in {:?} ({} diags)", t.elapsed(), parsed.diagnostics.len());
    }
}

/// Regression for the silent-output-drop: `-> (...)` on the line after the
/// parameter list used to be skipped entirely (param list never ate the
/// newline), leaving the mod with zero outputs and a cascade of WS002s.
#[test]
fn arrow_on_next_line_parses_outputs() {
    arm_watchdog(15, "arrow_on_next_line_parses_outputs");
    use wirescript::ast::TopDecl;

    let src = "mod F(a: int, b: int)\n    -> (x: float, y: float) {\n  out x = a\n  out y = b\n}\n";
    let parsed = wirescript::parse(src, "t.ws");
    assert!(parsed.diagnostics.is_empty(), "diags: {:?}", parsed.diagnostics);
    let TopDecl::Chip(c) = &parsed.ast.decls[0] else {
        panic!("expected mod/chip decl");
    };
    assert_eq!(c.outputs.len(), 2, "outputs should be parsed");

    // Multi-line output lists too.
    let src2 = "chip G(a: int) -> (x: float,\n                   y: float,\n) {\n  out x = a\n  out y = a\n}\n";
    let parsed2 = wirescript::parse(src2, "t.ws");
    assert!(parsed2.diagnostics.is_empty(), "diags: {:?}", parsed2.diagnostics);
    let TopDecl::Chip(c2) = &parsed2.ast.decls[0] else {
        panic!("expected chip decl");
    };
    assert_eq!(c2.outputs.len(), 2, "multi-line outputs should be parsed");
}

#[test]
fn orig_multiline_imports_parse_terminates() {
    arm_watchdog(15, "parse(multi-line imports)");
    for (name, src) in [("cursor", ORIG_CURSOR_WS), ("calibrate", ORIG_CALIBRATE_WS)] {
        let t = std::time::Instant::now();
        let parsed = wirescript::parse(src, name);
        eprintln!(
            "parse {name}: {:?}, {} decls, {} diags",
            t.elapsed(),
            parsed.ast.decls.len(),
            parsed.diagnostics.len()
        );
    }
}

#[test]
fn orig_files_full_lsp_surface_terminates() {
    arm_watchdog(60, "full LSP surface (original files)");
    let loader = full_mem_loader();
    lsp_surface("lib", BROKEN_LIB, "lib.ws", &loader, true);
    lsp_surface("cursor", ORIG_CURSOR_WS, "cursor.ws", &loader, true);
    lsp_surface("calibrate", ORIG_CALIBRATE_WS, "calibrate.ws", &loader, true);
}

#[test]
fn broken_lib_parse_terminates() {
    arm_watchdog(15, "parse(broken lib)");
    let t = std::time::Instant::now();
    let parsed = wirescript::parse(BROKEN_LIB, "lib.ws");
    eprintln!(
        "parse: {:?}, {} decls, {} diagnostics",
        t.elapsed(),
        parsed.ast.decls.len(),
        parsed.diagnostics.len()
    );
}

#[test]
fn broken_lib_typecheck_terminates() {
    arm_watchdog(20, "typecheck(broken lib)");
    let loader = mem_loader();
    let t = std::time::Instant::now();
    let resolved = resolve(BROKEN_LIB, "lib.ws", &loader);
    let rt = t.elapsed();
    let t2 = std::time::Instant::now();
    let tc = typecheck(&resolved.ast, "lib.ws");
    eprintln!(
        "resolve: {rt:?}, typecheck: {:?}, {} diags",
        t2.elapsed(),
        resolved.diagnostics.len() + tc.diagnostics.len()
    );
}

#[test]
fn broken_lib_estimates_terminate() {
    arm_watchdog(20, "collect_estimates(broken lib)");
    let loader = mem_loader();
    let resolved = resolve(BROKEN_LIB, "lib.ws", &loader);
    let tc = typecheck(&resolved.ast, "lib.ws");
    let t = std::time::Instant::now();
    let est = collect_estimates(&resolved.ast, &tc, "lib.ws");
    eprintln!("estimates: {:?}, {} entries", t.elapsed(), est.len());
}

#[test]
fn cursor_importing_broken_lib_full_lsp_path_terminates() {
    arm_watchdog(30, "full analyze(cursor.ws + broken lib)");
    let loader = mem_loader();
    let t = std::time::Instant::now();
    let _pre = wirescript::parse(CURSOR_WS, "cursor.ws");
    let resolved = resolve(CURSOR_WS, "cursor.ws", &loader);
    let rt = t.elapsed();
    let t2 = std::time::Instant::now();
    let tc = typecheck(&resolved.ast, "cursor.ws");
    let tt = t2.elapsed();
    let t3 = std::time::Instant::now();
    let est = collect_estimates(&resolved.ast, &tc, "cursor.ws");
    eprintln!(
        "resolve: {rt:?}, typecheck: {tt:?}, estimates: {:?} ({} entries)",
        t3.elapsed(),
        est.len()
    );
}

// ---------------------------------------------------------------------------
// 2026-07-08 incident: recursive chip/mod calls overflow the lowering stack.
//
// A named chip's body is lowered at its first call site
// (lower_chip_call_instance -> build_chip_module); the compiled template is
// only inserted into the cache AFTER that build returns. A chip whose body
// calls itself (directly or mutually) therefore always misses the cache and
// re-enters the build, recursing until the stack overflows and the process
// aborts. The LSP runs this on every keystroke via collect_estimates
// (estimate_handler compiles a synthetic chip from each handler body), so
// opening such a file killed the LSP outright. Inline mods hit the same
// recursion through lower_chip_call_inline expanding the body in-place.
// ---------------------------------------------------------------------------

/// Minimised shape of the crashing file: a chip declared inside an `on`
/// handler, calling itself from its own body, plus a kick-off call.
const RECURSIVE_CHIP_WS: &str = r#"
array items: controller[]

on RoundStart {
  var index: int = 0
  chip loop() {
    Greet(items[index])
    index += 1
    if items[index] { loop() }
  }
  if items[index] { loop() }
}

chip Greet(who: controller) {
  ShowStatusMessage(who, "hello")
}
"#;

/// Mutual recursion through two top-level chips.
const MUTUAL_RECURSION_WS: &str = r#"
chip Ping(n: int) { Pong(n + 1) }
chip Pong(n: int) { Ping(n + 1) }
on RoundStart { Ping(0) }
"#;

/// Self-recursive inline mod (expanded at the call site, same blowup).
const RECURSIVE_MOD_WS: &str = r#"
mod Again(n: int) { Again(n + 1) }
on RoundStart { Again(0) }
"#;

fn lower_source(src: &str, file: &str) -> wirescript::lower::LowerResult {
    let loader = MemLoader { files: HashMap::default() };
    let resolved = resolve(src, file, &loader);
    let tc = typecheck(&resolved.ast, file);
    wirescript::lower::lower(wirescript::lower::LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file,
        module_name: None,
        template_cache: std::sync::Arc::new(wirescript::template_cache::TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
    })
}

fn assert_recursion_error(label: &str, src: &str) {
    let r = lower_source(src, "rec.ws");
    assert!(
        r.diagnostics.iter().any(|d| d.severity == wirescript::Severity::Error
            && d.code == "WS020"
            && d.message.contains("recursive")),
        "{label}: expected a WS020 recursive-call error, got: {:?}",
        r.diagnostics
    );
}

#[test]
fn recursive_chip_call_lowers_with_diagnostic() {
    arm_watchdog(20, "lower(recursive chip in handler)");
    assert_recursion_error("handler-nested chip", RECURSIVE_CHIP_WS);
}

#[test]
fn mutually_recursive_chips_lower_with_diagnostic() {
    arm_watchdog(20, "lower(mutually recursive chips)");
    assert_recursion_error("mutual chips", MUTUAL_RECURSION_WS);
}

#[test]
fn recursive_inline_mod_lowers_with_diagnostic() {
    arm_watchdog(20, "lower(recursive inline mod)");
    assert_recursion_error("inline mod", RECURSIVE_MOD_WS);
}

/// The exact LSP analyze path over the crashing shape (this is what ran on
/// every keystroke and took the server down).
#[test]
fn recursive_chip_estimates_terminate() {
    arm_watchdog(20, "collect_estimates(recursive chip)");
    let loader = MemLoader { files: HashMap::default() };
    let resolved = resolve(RECURSIVE_CHIP_WS, "rec.ws", &loader);
    let tc = typecheck(&resolved.ast, "rec.ws");
    let est = collect_estimates(&resolved.ast, &tc, "rec.ws");
    eprintln!("estimates: {} entries", est.len());
}

/// Full LSP request surface (symbols, estimates, hints, format, hover sweep).
#[test]
fn recursive_chip_full_lsp_surface_terminates() {
    arm_watchdog(30, "lsp_surface(recursive chip)");
    let loader = MemLoader { files: HashMap::default() };
    lsp_surface("recursive-chip", RECURSIVE_CHIP_WS, "rec.ws", &loader, true);
}
