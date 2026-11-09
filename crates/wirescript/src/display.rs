//! `@display` and `@seat`: annotations that place bricks around the program's
//! chip.
//!
//! `@display(width = W, height = H) var screen` is a `color[]` of `W * H`
//! pixels. [`rewrite`] adds the code that draws it: a handler on a container
//! change detector on `screen` that loops over the pixels to rebuild one
//! string per row (`█` per pixel, a `<color="RRGGBB">` tag where the color
//! changes, a space for a pixel under half alpha), and one `string` var per
//! row that the row's Text Decal reads. That code is parsed after the end of the entry file, so
//! its ranges never meet the user's; lowering moves it onto the display's
//! back chip.
//!
//! `@seat(...) in p1` reads a placed seat: a `Seat` record of `occupied` and
//! `character`.

use std::sync::Arc;

use crate::ast::{
    BrickAnn, CallArg, DisplayBlock, Expr, ImportKind, InDecl, RecordTypeField, Script, TopDecl,
    TypeAliasDecl, TypeExpr, VarDecl,
};
use crate::diagnostic::{Diagnostic, Pos, Severity, SourceRange};
use crate::exterior::Side;

/// Every name [`rewrite`] declares starts with this.
pub const PREFIX: &str = "__display_";

/// Whether `name` is one [`rewrite`] declared.
pub fn is_display_name(name: &str) -> bool {
    name.starts_with(PREFIX)
}

/// The longest text one Text Decal holds.
pub const MAX_ROW_CHARS: usize = 32_767;
/// The most characters one pixel costs: a `<color="RRGGBB">` tag and a `█`.
const MAX_PIXEL_CHARS: usize = 17;
/// The widest display whose every row fits one Text Decal.
pub const MAX_WIDTH: u32 = (MAX_ROW_CHARS / MAX_PIXEL_CHARS) as u32;
/// The tallest display: one Text Decal, row var and copy per row.
pub const MAX_HEIGHT: u32 = 512;
/// The longest side a display's panel may have, in grid units (1000 studs):
/// a row is one brick that long.
pub const MAX_PANEL: u32 = 10_000;
/// The most pixels a display has: its rebuild runs one loop iteration a
/// pixel within a tick, under the game's 100,000 invocations a tick.
pub const MAX_PIXELS: u32 = 65_536;

const DISPLAY_CODE: &str = "WS127";
const SEAT_CODE: &str = "WS128";

/// The font a display's rows are drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayFont {
    IosevkaTerm,
    MonaspaceArgon,
}

impl DisplayFont {
    /// The `BrickFontDescriptor` asset.
    pub fn asset(self) -> &'static str {
        match self {
            Self::IosevkaTerm => "IosevkaTerm",
            Self::MonaspaceArgon => "MonaspaceArgon",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "IosevkaTerm" => Some(Self::IosevkaTerm),
            "MonaspaceArgon" => Some(Self::MonaspaceArgon),
            _ => None,
        }
    }
}

/// A checked `@display`.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplaySpec {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Grid units per pixel: `size * 10`, an even integer.
    pub pitch: i32,
    pub font: DisplayFont,
    pub border: bool,
    /// The side of the chip it stands on (`@top` unless a side annotation
    /// says otherwise), facing the chip.
    pub side: Side,
    /// `offset`, in grid units: to the display's right, out toward its
    /// viewer, and up.
    pub offset: [i32; 3],
}

/// `SeatMode` values, by the names the game gives them.
pub const SEAT_MODES: &[&str] = &["Auto", "Ground", "GroundUnlocked", "Air"];

/// The fonts a display draws in.
pub const FONTS: &[&str] = &["IosevkaTerm", "MonaspaceArgon"];

/// What an option of `@display` or `@seat` takes, for completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionValue {
    Int,
    Number,
    Bool,
    /// One of these strings.
    OneOf(&'static [&'static str]),
    /// Any string.
    Text,
    /// A call of this form, with number literals.
    Call(&'static str),
}

/// One option of `@display` or `@seat`: its name, what it takes and what it
/// does.
#[derive(Clone, Copy, Debug)]
pub struct AnnOption {
    pub name: &'static str,
    pub value: OptionValue,
    pub doc: &'static str,
}

const fn option(name: &'static str, value: OptionValue, doc: &'static str) -> AnnOption {
    AnnOption { name, value, doc }
}

/// The options `@display` takes.
pub const DISPLAY_OPTIONS: &[AnnOption] = &[
    option(
        "width",
        OptionValue::Int,
        "pixels across (required, up to 1927)",
    ),
    option(
        "height",
        OptionValue::Int,
        "pixels down (required, up to 512)",
    ),
    option(
        "size",
        OptionValue::Number,
        "a pixel's edge in studs, a multiple of 0.2 (default 1.0)",
    ),
    option(
        "font",
        OptionValue::OneOf(FONTS),
        "the rows' font (default \"IosevkaTerm\")",
    ),
    option(
        "border",
        OptionValue::Bool,
        "a one-pixel border around the panel (default true)",
    ),
    option(
        "offset",
        OptionValue::Call("Vec(0, 0, 0)"),
        "moves the display, in studs: to its right, out toward its viewer, and up",
    ),
];

/// The options `@seat` takes.
pub const SEAT_OPTIONS: &[AnnOption] = &[
    option(
        "label",
        OptionValue::Text,
        "the prompt a player about to sit sees",
    ),
    option(
        "mode",
        OptionValue::OneOf(SEAT_MODES),
        "the seat's controls (SeatMode)",
    ),
    option(
        "camera",
        OptionValue::Call("Rotation(0.0, 0.0, 0.0)"),
        "the third-person camera's rotation: pitch, yaw, roll",
    ),
    option(
        "exit",
        OptionValue::Call("Vec(0, 0, 0)"),
        "where a player stands up, in grid units from the seat",
    ),
];

/// The names of `options`.
fn names(options: &[AnnOption]) -> Vec<&'static str> {
    options.iter().map(|o| o.name).collect()
}

/// A checked `@seat`: the seat fields its options set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeatSpec {
    /// `PromptCustomLabel`.
    pub label: Option<String>,
    /// `SeatMode`, as its enum value.
    pub mode: Option<u8>,
    /// `ThirdPersonCameraRotation`: pitch, yaw, roll in degrees.
    pub camera: Option<[f64; 3]>,
    /// `ExitOffset`, in grid units.
    pub exit: Option<[i32; 3]>,
}

/// A constant option value as written.
enum Value<'a> {
    Int(i64),
    Float(f64),
    Str(&'a str),
    Bool(bool),
    Call(&'a str, Vec<f64>),
}

fn number(e: &Expr) -> Option<f64> {
    match e {
        Expr::IntLit { value, .. } => Some(*value as f64),
        Expr::FloatLit { value, .. } => Some(*value),
        Expr::UnOp { op, operand, .. } if op == "-" => number(operand).map(|v| -v),
        _ => None,
    }
}

fn value(e: &Expr) -> Option<Value<'_>> {
    match e {
        Expr::IntLit { value, .. } => Some(Value::Int(*value)),
        Expr::FloatLit { value, .. } => Some(Value::Float(*value)),
        Expr::UnOp { op, operand, .. } if op == "-" => match value(operand)? {
            Value::Int(v) => Some(Value::Int(-v)),
            Value::Float(v) => Some(Value::Float(-v)),
            _ => None,
        },
        Expr::StringLit { value, .. } => Some(Value::Str(value)),
        Expr::BoolLit { value, .. } => Some(Value::Bool(*value)),
        Expr::Call { callee, args, .. } => {
            let Expr::Ident { name, .. } = callee.as_ref() else {
                return None;
            };
            let nums = args
                .iter()
                .map(|a| match a {
                    CallArg::Positional(e) => number(e),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Value::Call(name, nums))
        }
        _ => None,
    }
}

/// Each `name = value` option of `ann`, reporting a positional or repeated
/// one, or a name not in `known`.
fn options<'a>(
    ann: &'a BrickAnn,
    what: &str,
    code: &str,
    known: &[&str],
    diags: &mut Vec<Diagnostic>,
) -> Vec<(&'a str, &'a Expr)> {
    let mut out: Vec<(&str, &Expr)> = Vec::new();
    for arg in &ann.args {
        match arg {
            CallArg::Named {
                name,
                value,
                name_range,
            } => {
                if !known.contains(&name.as_str()) {
                    diags.push(Diagnostic::error(
                        code,
                        format!(
                            "{what} has no option `{name}`; expected {}",
                            known.join(", ")
                        ),
                        name_range.clone(),
                    ));
                } else if out.iter().any(|(n, _)| n == name) {
                    diags.push(Diagnostic::error(
                        code,
                        format!("{what} option `{name}` is given twice"),
                        name_range.clone(),
                    ));
                } else {
                    out.push((name, value));
                }
            }
            CallArg::Positional(e) | CallArg::Spread(e) => diags.push(Diagnostic::error(
                code,
                format!("{what} options are named: `name = value`"),
                e.range().clone(),
            )),
        }
    }
    out
}

/// Whether `t` is written `color[]`.
fn is_color_array(t: &TypeExpr) -> bool {
    matches!(t, TypeExpr::Array { inner, .. }
        if matches!(inner.as_ref(), TypeExpr::Name { name, .. } if name.eq_ignore_ascii_case("color")))
}

/// Check a `@display` var. Reports each problem into `diags`; `None` when
/// any was found.
pub fn display_spec(v: &VarDecl, diags: &mut Vec<Diagnostic>) -> Option<DisplaySpec> {
    let ann = v.display.as_ref()?;
    let before = diags.len();
    let bad = |diags: &mut Vec<Diagnostic>, msg: String, e: &Expr| {
        diags.push(Diagnostic::error(DISPLAY_CODE, msg, e.range().clone()));
    };
    if !v.typ.as_ref().is_some_and(is_color_array) {
        let range = v.typ.as_ref().map_or(&v.range, TypeExpr::range).clone();
        diags.push(Diagnostic::error(
            DISPLAY_CODE,
            format!(
                "a @display var is a `color[]`: write `var {}: color[]` or leave the type out",
                v.name
            ),
            range,
        ));
    }
    if let Some(init) = &v.init {
        bad(
            diags,
            "a @display var starts with every pixel transparent and takes no initializer"
                .to_string(),
            init,
        );
    }
    let known = names(DISPLAY_OPTIONS);
    let opts = options(ann, "@display", DISPLAY_CODE, &known, diags);
    let mut spec = DisplaySpec {
        name: v.name.clone(),
        width: 0,
        height: 0,
        pitch: 10,
        font: DisplayFont::IosevkaTerm,
        border: true,
        side: ann
            .side
            .and_then(|s| Side::parse(s.as_str()))
            .unwrap_or(Side::Top),
        offset: [0, 0, 0],
    };
    for (name, e) in &opts {
        match (*name, value(e)) {
            ("width" | "height", Some(Value::Int(n))) => {
                let max = if *name == "width" {
                    MAX_WIDTH
                } else {
                    MAX_HEIGHT
                };
                if n < 1 || n > i64::from(max) {
                    bad(diags, format!("@display {name} must be from 1 to {max}"), e);
                } else if *name == "width" {
                    spec.width = n as u32;
                } else {
                    spec.height = n as u32;
                }
            }
            ("size", Some(Value::Int(_) | Value::Float(_))) => {
                let size = number(e).unwrap_or_default();
                let pitch = (size * 10.0).round();
                if (size * 10.0 - pitch).abs() > 1e-6
                    || pitch < 2.0
                    || pitch > 200.0
                    || pitch as i64 % 2 != 0
                {
                    bad(
                        diags,
                        "@display size is a pixel's edge in studs: a multiple of 0.2 from 0.2 to 20"
                            .to_string(),
                        e,
                    );
                } else {
                    spec.pitch = pitch as i32;
                }
            }
            ("font", Some(Value::Str(s))) => match DisplayFont::parse(s) {
                Some(f) => spec.font = f,
                None => bad(
                    diags,
                    format!(
                        "@display font `{s}` is not one of \"IosevkaTerm\", \"MonaspaceArgon\""
                    ),
                    e,
                ),
            },
            ("border", Some(Value::Bool(b))) => spec.border = b,
            ("offset", Some(Value::Call("Vec", n))) if n.len() == 3 => {
                spec.offset = [n[0], n[1], n[2]].map(|studs| (studs * 10.0).round() as i32);
            }
            (name, _) => {
                let want = match name {
                    "width" | "height" => "an integer literal",
                    "size" => "a number literal",
                    "font" => "a string literal",
                    "offset" => "`Vec(right, out, up)` in studs, with number literals",
                    _ => "`true` or `false`",
                };
                bad(diags, format!("@display {name} must be {want}"), e);
            }
        }
    }
    for required in ["width", "height"] {
        if !opts.iter().any(|(n, _)| *n == required) {
            diags.push(Diagnostic::error(
                DISPLAY_CODE,
                format!("@display needs a `{required}`: @display(width = 32, height = 20)"),
                ann.range.clone(),
            ));
        }
    }
    let side = spec.width.max(spec.height) * spec.pitch as u32;
    if diags.len() == before && side > MAX_PANEL {
        diags.push(Diagnostic::error(
            DISPLAY_CODE,
            format!(
                "@display is {} studs across at this size; a side may be at most {} studs: \
                 give fewer pixels or a smaller `size`",
                side / 10,
                MAX_PANEL / 10
            ),
            ann.range.clone(),
        ));
    }
    let pixels = spec.width * spec.height;
    if diags.len() == before && pixels > MAX_PIXELS {
        diags.push(Diagnostic::error(
            DISPLAY_CODE,
            format!(
                "@display has {pixels} pixels; at most {MAX_PIXELS} redraw within a tick \
                 (256 x 256, say)"
            ),
            ann.range.clone(),
        ));
    }
    (diags.len() == before).then_some(spec)
}

/// Check a `@seat` input. Reports each problem into `diags`; `None` when any
/// was found.
pub fn seat_spec(i: &InDecl, diags: &mut Vec<Diagnostic>) -> Option<SeatSpec> {
    let ann = i.seat.as_ref()?;
    let before = diags.len();
    let bad = |diags: &mut Vec<Diagnostic>, msg: String, e: &Expr| {
        diags.push(Diagnostic::error(SEAT_CODE, msg, e.range().clone()));
    };
    if !matches!(&i.typ, TypeExpr::Name { name, .. } if name == "Seat") {
        diags.push(Diagnostic::error(
            SEAT_CODE,
            format!(
                "a @seat input is a `Seat`: write `in {}: Seat` or leave the type out",
                i.name
            ),
            i.typ.range().clone(),
        ));
    }
    let known = names(SEAT_OPTIONS);
    let mut spec = SeatSpec::default();
    for (name, e) in options(ann, "@seat", SEAT_CODE, &known, diags) {
        match (name, value(e)) {
            ("label", Some(Value::Str(s))) => spec.label = Some(s.to_string()),
            ("mode", Some(Value::Str(s))) => match SEAT_MODES.iter().position(|m| *m == s) {
                Some(n) => spec.mode = Some(n as u8),
                None => bad(
                    diags,
                    format!(
                        "@seat mode `{s}` is not one of {}",
                        SEAT_MODES
                            .iter()
                            .map(|m| format!("\"{m}\""))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    e,
                ),
            },
            ("camera", Some(Value::Call("Rotation", n))) if n.len() == 3 => {
                spec.camera = Some([n[0], n[1], n[2]]);
            }
            ("exit", Some(Value::Call("Vec", n))) if n.len() == 3 => {
                spec.exit = Some([n[0] as i32, n[1] as i32, n[2] as i32]);
            }
            (name, _) => {
                let want = match name {
                    "label" => "a string literal",
                    "mode" => "a string literal such as \"Ground\"",
                    "camera" => "`Rotation(pitch, yaw, roll)` with number literals",
                    _ => "`Vec(x, y, z)` with number literals",
                };
                bad(diags, format!("@seat {name} must be {want}"), e);
            }
        }
    }
    (diags.len() == before).then_some(spec)
}

/// The name of display `name`'s row `r` var.
pub fn row_var(name: &str, r: u32) -> String {
    format!("{PREFIX}{name}_row{r}")
}

/// The code that draws display `d`: the rows array, one var per row, and the
/// change handler whose loops rebuild the rows and copy each into its var.
fn display_code(d: &DisplaySpec) -> String {
    let DisplaySpec {
        name,
        width: w,
        height: h,
        ..
    } = d;
    let p = format!("{PREFIX}{name}");
    let mut s = format!("\n@label(\"rows\") var {p}_rows: string[]\n");
    for r in 0..*h {
        s += &format!(
            "@label(\"row {r}\") var {}: string = \"\"\n",
            row_var(name, r)
        );
    }
    s += &format!(
        r#"on ContainerChange({name}) {{
  {p}_rows.clear()
  for y in 0..{h} {{
    var line: string = ""
    var last: string = ""
    for x in 0..{w} {{
      let c = {name}[y * {w} + x]
      if c.a < 0.5 {{
        line = line .. " "
      }} else {{
        let hex = c.ToHex()
        if hex != last {{
          line = line .. "<color=\"" .. hex .. "\">"
          last = hex
        }}
        line = line .. "█"
      }}
    }}
    {p}_rows.push(line)
  }}
"#
    );
    for r in 0..*h {
        s += &format!("  {} = {p}_rows[{r}]\n", row_var(name, r));
    }
    s += "}\n";
    s
}

/// Report each `@display` and `@seat` of an imported file: they place bricks
/// around the program's own chip, so only the entry file declares them.
pub(crate) fn reject_imported(ast: &Script, diags: &mut Vec<Diagnostic>) {
    for d in &ast.decls {
        let (code, what, ann) = match d {
            TopDecl::Var(v) => (DISPLAY_CODE, "@display", v.display.as_ref()),
            TopDecl::In(i) => (SEAT_CODE, "@seat", i.seat.as_ref()),
            _ => continue,
        };
        if let Some(ann) = ann {
            diags.push(Diagnostic::error(
                code,
                format!(
                    "{what} is only allowed in the file being compiled, not in an imported one"
                ),
                ann.range.clone(),
            ));
        }
    }
}

/// The builtin `Seat` record, `{ occupied: bool, character: character }`,
/// as a declaration at `range`.
fn seat_type(range: &SourceRange) -> TopDecl {
    let name = |n: &str| TypeExpr::Name {
        name: n.to_string(),
        range: range.clone(),
    };
    let field = |n: &str, t: &str| RecordTypeField {
        name: n.to_string(),
        typ: name(t),
        range: range.clone(),
    };
    TopDecl::TypeAlias(TypeAliasDecl {
        name: "Seat".to_string(),
        type_params: Vec::new(),
        typ: TypeExpr::Record {
            fields: vec![field("occupied", "bool"), field("character", "character")],
            range: range.clone(),
        },
        range: range.clone(),
    })
}

/// Whether the entry file declares a `Seat` of its own, or imports one by
/// name.
fn declares_seat(ast: &Script) -> bool {
    ast.decls.iter().any(|d| match d {
        TopDecl::TypeAlias(t) => t.name == "Seat",
        TopDecl::Enum(e) => e.name == "Seat",
        TopDecl::Chip(c) => c.name == "Seat",
        TopDecl::Import(imp) => match &imp.kind {
            ImportKind::Named(bindings) => bindings
                .iter()
                .any(|b| b.alias.as_deref().unwrap_or(&b.name) == "Seat"),
            _ => false,
        },
        _ => false,
    })
}

/// The name a user's top-level declaration gives, where it can collide with
/// one [`rewrite`] adds.
fn declared_name(d: &TopDecl) -> Option<(&str, &SourceRange)> {
    match d {
        TopDecl::Var(v) => Some((&v.name, &v.range)),
        TopDecl::In(i) => Some((&i.name, &i.range)),
        TopDecl::Chip(c) => Some((&c.name, &c.range)),
        TopDecl::TypeAlias(t) => Some((&t.name, &t.range)),
        _ => None,
    }
}

/// Check the entry file's `@display` and `@seat` declarations, and add the
/// code each display needs to `ast` (see the module docs), with the builtin
/// `Seat` type when the file names it. `clean` is whether the entry file
/// parsed without errors: the added code is only parsed after a clean file,
/// which cannot swallow it. `after` is where offsets other code made up
/// end (the `testing` rewrite's); the added code starts past it.
pub(crate) fn rewrite(
    ast: &mut Script,
    source: &str,
    file: &str,
    clean: bool,
    after: usize,
    diags: &mut Vec<Diagnostic>,
) {
    let mut code = String::new();
    let mut names = Vec::new();
    let mut seats = false;
    for d in &ast.decls {
        if let Some((name, range)) = declared_name(d)
            && is_display_name(name)
        {
            diags.push(Diagnostic::error(
                "WS093",
                format!(
                    "`{name}` starts with `__`, which is reserved for names the compiler \
                     makes; rename it"
                ),
                range.clone(),
            ));
        }
        match d {
            TopDecl::Var(v) if v.display.is_some() => {
                let Some(spec) = display_spec(v, diags) else {
                    continue;
                };
                if ast.flat {
                    let range = v.display.as_ref().map_or(&v.range, |a| &a.range);
                    diags.push(Diagnostic::error(
                        DISPLAY_CODE,
                        "@display cannot be used with @flat: its rebuild runs in loop planes, \
                         which @flat does not flatten",
                        range.clone(),
                    ));
                    continue;
                }
                names.push((spec.name.clone(), code.len()));
                code += &display_code(&spec);
            }
            TopDecl::In(i) if i.seat.is_some() => {
                seats = true;
                seat_spec(i, diags);
            }
            _ => {}
        }
    }
    let base = (source.len() + 1).max(after);
    let at = |offset: usize| SourceRange {
        file: Arc::from(file),
        start: Pos {
            offset,
            line: 0,
            col: 0,
        },
        end: Pos {
            offset,
            line: 0,
            col: 0,
        },
    };
    // Types register in declaration order, so `Seat` goes before the code
    // that names it.
    if (seats || source.contains("Seat")) && !declares_seat(ast) {
        ast.decls.insert(0, seat_type(&at(base + code.len())));
    }
    if code.is_empty() || !clean {
        return;
    }
    let pad = base - (source.len() + 1);
    let full = format!("{source}\n{}{code}", " ".repeat(pad));
    let parsed = crate::parser::parse(&full, file);
    debug_assert!(
        !parsed
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error && d.range.start.offset >= base),
        "generated display code must parse: {:?}",
        parsed.diagnostics
    );
    let added: Vec<TopDecl> = parsed
        .ast
        .decls
        .into_iter()
        .filter(|d| d.range().start.offset >= base)
        .collect();
    for (i, (name, start)) in names.iter().enumerate() {
        let end = names.get(i + 1).map_or(code.len(), |(_, s)| *s);
        let (start, end) = (base + start, base + end);
        let mut block = added
            .iter()
            .map(TopDecl::range)
            .filter(|r| (start..end).contains(&r.start.offset));
        if let Some(first) = block.next() {
            let last = block.last().unwrap_or(first);
            ast.display_blocks.push(DisplayBlock {
                name: name.clone(),
                range: SourceRange {
                    file: first.file.clone(),
                    start: first.start,
                    end: last.end,
                },
            });
        }
    }
    ast.decls.extend(added);
}

#[cfg(test)]
mod tests;
