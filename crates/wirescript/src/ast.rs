//!
//! Every node carries a [`SourceRange`] so diagnostics can attribute
//! errors back to source. The typechecker attaches resolved types later
//! by producing a parallel typed module — Phase 4 work.

use crate::diagnostic::SourceRange;

#[derive(Clone, Debug, Default)]
pub struct Script {
    pub decls: Vec<TopDecl>,
    pub range: SourceRange,
    /// A `///` doc block at the very top of the file, separated from the first
    /// declaration by a blank line — documents the module (root plane header)
    /// rather than the first declaration.
    pub module_doc: Option<String>,
    /// A `@nofold` at the very top of the file (after any module doc),
    /// separated from the first declaration by a blank line — marks the whole
    /// module's lowered output `_nofold` (mirrors the module-doc rule).
    pub no_fold: bool,
    /// A `@fold` at the very top of the file (after any module doc),
    /// separated from the first declaration by a blank line — opts the whole
    /// module into the certified constant-fold pass under `FoldMode::Auto`
    /// (mirrors the module-doc rule; same mechanics as `no_fold` above). If
    /// both `@fold` and `@nofold` are present, `no_fold` wins.
    pub fold: bool,
    /// A `@layout("…")` at the very top of the file (same placement rule as
    /// `fold` above) — names the placement engine to use. `None` leaves the
    /// choice to the compiler.
    pub layout: Option<LayoutName>,
    /// A `@flat` at the very top of the file (same placement rule as `fold`
    /// above) — every chip body is inlined into the module that instantiates
    /// it, so the program emits no microchip bricks and no child brick grids.
    /// Independent of `layout`; the two compose.
    pub flat: bool,
    /// A `@invisible` at the very top of the file (same placement rule as
    /// `fold` above) — the emitted top-level microchip shell is hidden,
    /// non-colliding, and carries no labels (root name, plane header, var
    /// tags, I/O gate labels). Independent of `flat`/`layout`.
    pub invisible: bool,
    /// A `@label(<expr>)` at the very top of the file (same blank-line
    /// placement rule as `fold` above) — labels the ROOT microchip rather than
    /// a declaration. A constant expression bakes static title text; a runtime
    /// expression wires the live value into the root chip's label. The
    /// expression may forward-reference declarations below it (resolved in a
    /// post-declaration pass), so `@label(x)` above `var x` is valid.
    pub module_label: Option<Expr>,
}

/// The engine a `@layout("…")` annotation names, spelled as the source spells
/// it. Distinct from [`crate::layout::LayoutMode`], which is the set of
/// engines: the default engine has no annotation spelling, and an engine the
/// compiler may pick on its own is not necessarily one a file may ask for.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LayoutName {
    /// `@layout("code")` — row per source line, indent per source column.
    Code,
    /// `@layout("cube")` — the compact 3D grid arrangement the compiler
    /// otherwise reaches for only on very large modules.
    Cube,
}

impl LayoutName {
    /// The spellings accepted inside `@layout(…)`, in the order a diagnostic
    /// should offer them.
    pub const ALL: [(&'static str, LayoutName); 2] =
        [("code", LayoutName::Code), ("cube", LayoutName::Cube)];

    pub fn parse(name: &str) -> Option<LayoutName> {
        Self::ALL.iter().find(|(s, _)| *s == name).map(|(_, m)| *m)
    }
}

/// Lexical facts about one source file that survive parsing: how far each
/// line is indented, and where its `//` comments are. The code-shaped
/// layout reproduces the source's own shape, so it needs the text's
/// geometry rather than the AST's.
#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    /// The file this map describes, spelled the way
    /// [`SourceRange::file`] spells it. Every line number below is in this
    /// file's numbering, so a consumer holding nodes from another file must
    /// compare `file` before indexing anything here.
    pub file: std::sync::Arc<str>,
    /// Per line (0-based index): the 0-based column of the line's first
    /// non-whitespace character. Blank and whitespace-only lines hold 0.
    /// A 1-based [`crate::diagnostic::Pos`] line indexes this as `line - 1`.
    pub line_indent: Vec<u32>,
    /// `//` line comments in source order.
    pub comments: Vec<SourceComment>,
}

/// One `//` line comment. `line`/`col` are 1-based like
/// [`crate::diagnostic::Pos`] and point at the leading `/`.
#[derive(Clone, Debug)]
pub struct SourceComment {
    pub line: u32,
    pub col: u32,
    /// Comment body with the `//`, one optional following space, and any
    /// trailing whitespace removed.
    pub text: String,
    /// True when the comment is the only thing on its line.
    pub own_line: bool,
    /// True when the comment sits inside an unclosed `[` — an array literal
    /// or a multi-line data table. These are the repetitive ones: a note per
    /// row of a table costs a brick per row and says the same thing each
    /// time, so the code layout leaves them out of the plane.
    pub in_array: bool,
}

impl SourceMap {
    /// Build the source map for `source` as if it were `file`, by running
    /// the lexer and keeping only its map. Compilation gets its map from
    /// the parse it already ran ([`crate::parser::ParseResult::source_map`],
    /// carried on through [`crate::resolve::ResolveResult`]); this is the
    /// shortcut for callers holding text and no parse, and it re-lexes.
    pub fn from_source(source: &str, file: &str) -> SourceMap {
        crate::lexer::lex(source, file).source_map
    }
}

// ---------- top-level declarations ----------

#[derive(Clone, Debug)]
pub enum ImportKind {
    All,
    Named(Vec<ImportBinding>),
    Namespace(String),
}

#[derive(Clone, Debug)]
pub struct ImportBinding {
    pub name: String,
    pub alias: Option<String>,
    /// Range of the effective identifier (alias if present, otherwise name).
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct ImportDecl {
    pub path: String,
    pub kind: ImportKind,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct NamespaceDecl {
    pub name: String,
    pub decls: Vec<TopDecl>,
    pub source_path: String,
    pub module_doc: Option<String>,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub enum TopDecl {
    Import(ImportDecl),
    Namespace(NamespaceDecl),
    Var(VarDecl),
    Array(ArrayDecl),
    Map(MapDecl),
    Buffer(BufferDecl),
    Fn(FnDecl),
    Chip(ChipDecl),
    AnonChip(AnonChipDecl),
    Event(EventDecl),
    In(InDecl),
    Out(OutBinding),
    Handler(Handler),
    Let(LetDecl),
    Await(AwaitStmt),
    Assign(Assign),
    If(If),
    ExprStmt(ExprStmt),
    TypeAlias(TypeAliasDecl),
}

impl TopDecl {
    pub fn range(&self) -> &SourceRange {
        match self {
            TopDecl::Import(d) => &d.range,
            TopDecl::Namespace(d) => &d.range,
            TopDecl::Var(d) => &d.range,
            TopDecl::Array(d) => &d.range,
            TopDecl::Map(d) => &d.range,
            TopDecl::Buffer(d) => &d.range,
            TopDecl::Fn(d) => &d.range,
            TopDecl::Chip(d) => &d.range,
            TopDecl::AnonChip(d) => &d.range,
            TopDecl::Event(d) => &d.range,
            TopDecl::In(d) => &d.range,
            TopDecl::Out(d) => &d.range,
            TopDecl::Handler(d) => &d.range,
            TopDecl::Let(d) => &d.range,
            TopDecl::Await(d) => &d.range,
            TopDecl::Assign(d) => &d.range,
            TopDecl::If(d) => &d.range,
            TopDecl::ExprStmt(d) => &d.range,
            TopDecl::TypeAlias(d) => &d.range,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TypeAliasDecl {
    pub name: String,
    /// Declaration-side generic type parameters: `type Pair<T> = { a: T, b: T }`.
    /// Empty for non-generic aliases.
    pub type_params: Vec<TypeParam>,
    pub typ: TypeExpr,
    pub range: SourceRange,
}

/// A generic type parameter on a `mod`/`chip`/`type` declaration, e.g. `T` or
/// `T: Numeric`. `bound` is the constraint TypeExpr (a named class like
/// `Numeric`, or a union like `int | vector`); `None` = unbounded (= `Variant`).
#[derive(Clone, Debug)]
pub struct TypeParam {
    pub name: String,
    pub bound: Option<TypeExpr>,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct VarDecl {
    pub name: String,
    pub typ: Option<TypeExpr>,
    pub init: Option<Expr>,
    pub is_static: bool,
    /// `@nofold`: every IR node lowered from this declaration's subtree
    /// carries the `_nofold` pseudo-property (the fold pass skips it).
    pub no_fold: bool,
    /// `@label("…")` display-text override for the var's floating label.
    pub label: Option<String>,
    /// `@label(expr)` — a compile-time-constant expression form of the
    /// above; folded to text at lowering. At most one of `label` /
    /// `label_expr` is ever set.
    pub label_expr: Option<Expr>,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct ArrayDecl {
    pub name: String,
    pub element_type: TypeExpr,
    /// Initial elements: `array foo: int[] = [1, 2, 3]`. Empty when no
    /// initializer is given. At top level every element must be a literal
    /// (`Item` with a literal expr); spreads / non-literals are rejected.
    pub init: Vec<ArrayElem>,
    pub range: SourceRange,
}

/// `map name: Map<K, V>` — a keyed variable collection (parallels `ArrayDecl`).
#[derive(Clone, Debug)]
pub struct MapDecl {
    pub name: String,
    pub key_type: TypeExpr,
    pub value_type: TypeExpr,
    /// Optional literal initializer: `map m: Map<int, int> = { 1 => 2 }`.
    pub init: Option<Expr>,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct BufferDecl {
    pub name: String,
    /// Optional explicit type annotation; useful for self-feedback buffers.
    pub typ: Option<TypeExpr>,
    pub init: Expr,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub typ: TypeExpr,
    pub pattern: Option<ParamPattern>,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub enum ParamPattern {
    Record {
        fields: Vec<RecordDestructField>,
        rest: Option<String>,
    },
    Tuple {
        names: Vec<String>,
        rest: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub struct NamedOutput {
    pub name: String,
    pub typ: TypeExpr,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    /// Expression-bodied: `fn foo(x: int) -> int = x + 1`
    pub body: Expr,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct ChipDecl {
    pub name: String,
    /// Declaration-side generic type parameters: `mod pick<T>(...)` /
    /// `chip Foo<T: Numeric>(...)`. Empty for non-generic decls. Covers `mod`
    /// too (a `mod` is a `ChipDecl` with `inline: true`).
    pub type_params: Vec<TypeParam>,
    pub inputs: Vec<Param>,
    pub outputs: Vec<NamedOutput>,
    pub body: Block,
    pub range: SourceRange,
    /// When true, always expanded inline at call sites (no physical
    /// microchip). Set by the `mod` keyword.
    pub inline: bool,
    /// `@label("…")` display-text override for the chip's labels and header.
    pub label: Option<String>,
    /// `@label(expr)` — a compile-time-constant expression form of the
    /// above; folded to text at lowering. At most one of `label` /
    /// `label_expr` is ever set.
    pub label_expr: Option<Expr>,
    /// `@closed`: emit this chip's inner grid collapsed.
    pub closed: bool,
    /// `@nofold`: every IR node lowered from this declaration's subtree
    /// carries the `_nofold` pseudo-property (the fold pass skips it).
    pub no_fold: bool,
}

impl ChipDecl {
    /// True when the first parameter is the receiver marker `self`, making this
    /// mod/chip callable with method syntax: `v.method(args)` desugars to
    /// `method(v, args)`. `self` is otherwise an ordinary parameter of its
    /// declared type (it is not a magic value). A destructured first parameter
    /// (`{ … }: T`) is never a receiver.
    pub fn is_self_receiver(&self) -> bool {
        self.inputs
            .first()
            .is_some_and(|p| p.name == "self" && p.pattern.is_none())
    }
}

/// Anonymous chip: `chip { body }` — shares parent scope, creates a
/// physical microchip grid for visual organization. Can have `in`/`out`
/// declarations inside the body for explicit I/O ports.
#[derive(Clone, Debug)]
pub struct AnonChipDecl {
    pub open: bool,
    pub body: Block,
    pub range: SourceRange,
    /// `@label("…")` display-text override for the chip's labels and header.
    pub label: Option<String>,
    /// `@label(expr)` — a compile-time-constant expression form of the
    /// above; folded to text at lowering. At most one of `label` /
    /// `label_expr` is ever set.
    pub label_expr: Option<Expr>,
    /// `@closed`: emit this chip's inner grid collapsed.
    pub closed: bool,
}

/// `event foo = Trigger` or `event foo = on Trigger { ... }`
#[derive(Clone, Debug)]
pub struct EventDecl {
    pub name: String,
    pub source: Expr,
    pub captured_body: Option<Block>,
    /// `@nofold`: every IR node lowered from this declaration's subtree
    /// carries the `_nofold` pseudo-property (the fold pass skips it).
    pub no_fold: bool,
    pub range: SourceRange,
}

/// Side of the compiled microchip that a port's outer rerouter is placed on
/// (`@left` / `@right` / `@top` / `@bottom` annotation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortSide {
    Left,
    Right,
    Top,
    Bottom,
}

impl PortSide {
    pub fn from_word(w: &str) -> Option<Self> {
        match w {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "top" => Some(Self::Top),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

#[derive(Clone, Debug)]
pub struct InDecl {
    pub name: String,
    pub typ: TypeExpr,
    pub side: Option<PortSide>,
    /// `@label("…")` display-text override for the port's floating label.
    pub label: Option<String>,
    /// `@label(expr)` — a compile-time-constant expression form of the
    /// above; folded to text at lowering. At most one of `label` /
    /// `label_expr` is ever set.
    pub label_expr: Option<Expr>,
    /// `@invisible`: the port's rerouter is emitted hidden.
    pub invisible: bool,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct LetDecl {
    pub binding: LetBinding,
    pub typ: Option<TypeExpr>,
    pub value: Expr,
    /// `@nofold`: every IR node lowered from this declaration's subtree
    /// carries the `_nofold` pseudo-property (the fold pass skips it).
    pub no_fold: bool,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub enum LetBinding {
    Ident {
        name: String,
        range: SourceRange,
    },
    Tuple {
        names: Vec<String>,
        rest: Option<String>,
        range: SourceRange,
    },
    Record {
        names: Vec<String>,
        range: SourceRange,
    },
    RecordDestruct {
        fields: Vec<RecordDestructField>,
        range: SourceRange,
    },
}

#[derive(Clone, Debug)]
pub enum RecordDestructField {
    Named {
        name: String,
        alias: Option<String>,
        range: SourceRange,
    },
    Rest {
        name: String,
        range: SourceRange,
    },
}

#[derive(Clone, Debug)]
pub struct HandlerParam {
    pub name: String,
    /// Optional type annotation (`a: int`). Events whose data-output types are
    /// declared by the handler — Custom Event — use it to type their output
    /// ports; `None` for events with fixed data types (RoundStart, etc.).
    pub ty: Option<TypeExpr>,
    /// Range of the param name (for diagnostics).
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct Handler {
    pub trigger: Trigger,
    /// `on Event(a, b) { ... }` — params that bind the event's data outputs
    /// (e.g. `controller`, `arguments`), optionally typed (`on CustomEvent(a:
    /// int, b: float)`).
    pub params: Vec<HandlerParam>,
    /// Literal/named config args that configure the event gate itself, e.g.
    /// `on ChatCommand("greet", Description = "Greets you") { ... }`. These map
    /// to the gate's data-struct fields (not output bindings).
    pub config: Vec<HandlerConfigArg>,
    pub body: Block,
    /// `@nofold`: every IR node lowered from this declaration's subtree
    /// carries the `_nofold` pseudo-property (the fold pass skips it).
    pub no_fold: bool,
    pub range: SourceRange,
}

/// A config argument on an event handler trigger. Positional args fill the
/// event's config fields in order; named args target a field by name.
#[derive(Clone, Debug)]
pub enum HandlerConfigArg {
    Positional(Expr),
    Named { name: String, value: Expr },
}

#[derive(Clone, Debug)]
pub enum Trigger {
    Ident {
        name: String,
        range: SourceRange,
    },
    Field {
        obj: String,
        field: String,
        range: SourceRange,
    },
    Not {
        inner: Box<Trigger>,
        range: SourceRange,
    },
    Union {
        parts: Vec<Trigger>,
        range: SourceRange,
    },
}

// ---------- type expressions ----------

#[derive(Clone, Debug)]
pub enum TypeExpr {
    /// `int`, `bool`, `entity`, chip type name, …
    Name { name: String, range: SourceRange },
    /// `ref T`
    Ref {
        inner: Box<TypeExpr>,
        range: SourceRange,
    },
    /// `T[]`
    Array {
        inner: Box<TypeExpr>,
        range: SourceRange,
    },
    /// `(A, B, C)`
    Tuple {
        fields: Vec<TypeExpr>,
        range: SourceRange,
    },
    /// `A | B | C`
    Union {
        options: Vec<TypeExpr>,
        range: SourceRange,
    },
    /// `{ field: Type, ... }` — record type
    Record {
        fields: Vec<RecordTypeField>,
        range: SourceRange,
    },
    /// A generic application `Name<Arg, ...>` (e.g. `Map<string, int>`).
    /// `Array<V>` / `Ref<V>` are desugared to `Array` / `Ref` at parse time, so
    /// this carries the remaining generics (currently `Map`) resolved by
    /// `type_of_type_expr`.
    Generic {
        name: String,
        args: Vec<TypeExpr>,
        range: SourceRange,
    },
}

impl TypeExpr {
    pub fn range(&self) -> &SourceRange {
        match self {
            TypeExpr::Name { range, .. }
            | TypeExpr::Ref { range, .. }
            | TypeExpr::Array { range, .. }
            | TypeExpr::Tuple { range, .. }
            | TypeExpr::Union { range, .. }
            | TypeExpr::Record { range, .. }
            | TypeExpr::Generic { range, .. } => range,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RecordTypeField {
    pub name: String,
    pub typ: TypeExpr,
    pub range: SourceRange,
}

// ---------- statements ----------

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Assign(Assign),
    Emit(Emit),
    Await(AwaitStmt),
    If(If),
    In(InDecl),
    Let(LetDecl),
    OutBinding(OutBinding),
    ExprStmt(ExprStmt),
    Var(VarDecl),
    Buffer(BufferDecl),
    Array(ArrayDecl),
    Map(MapDecl),
    Handler(Handler),
    AnonChip(AnonChipDecl),
    ChipDecl(ChipDecl),
    Return {
        value: Option<Expr>,
        range: SourceRange,
    },
}

impl Stmt {
    pub fn range(&self) -> &SourceRange {
        match self {
            Stmt::Assign(d) => &d.range,
            Stmt::Emit(d) => &d.range,
            Stmt::Await(d) => &d.range,
            Stmt::If(d) => &d.range,
            Stmt::In(d) => &d.range,
            Stmt::Let(d) => &d.range,
            Stmt::OutBinding(d) => &d.range,
            Stmt::ExprStmt(d) => &d.range,
            Stmt::Var(d) => &d.range,
            Stmt::Buffer(d) => &d.range,
            Stmt::Array(d) => &d.range,
            Stmt::Map(d) => &d.range,
            Stmt::Handler(d) => &d.range,
            Stmt::AnonChip(d) => &d.range,
            Stmt::ChipDecl(d) => &d.range,
            Stmt::Return { range, .. } => range,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Assign {
    pub target: Expr,
    pub value: Expr,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct Emit {
    pub name: String,
    pub value: Option<Expr>,
    /// `buffer(delay, hold)` modifier: routes this emit's exec through a
    /// Buffer gate (the tick-crossing barrier that makes loop back-edges
    /// legal). `None` for a plain immediate emit.
    pub buffer: Option<BufferSpec>,
    pub range: SourceRange,
}

/// The `buffer(delay[, hold])` spec on an emit (or bare `buffer emit` — one
/// tick). `delay` maps to the Buffer gate's `TicksToWait`/`SecondsToWait`
/// (`None` = 1 tick), `hold` to `ZeroTicksToWait`/`ZeroSecondsToWait` (how
/// long the output stays up after the input drops; gate default `-1` = same
/// as delay). An `s` unit suffix selects the seconds gate over the ticks gate.
#[derive(Clone, Debug)]
pub struct BufferSpec {
    pub delay: Option<Expr>,
    pub hold: Option<Expr>,
    pub seconds: bool,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct AwaitStmt {
    pub binding: Option<String>,
    /// `let { a, b: alias } = await sig` — record-destructured payload fields
    /// as `(field, local name)` pairs. Each field reads the signal's ferried
    /// payload store of that name.
    pub destructure: Option<Vec<(String, String)>>,
    pub value_expr: Option<Expr>,
    pub exec_expr: Expr,
    /// `@nofold`: every IR node lowered from this declaration's subtree
    /// carries the `_nofold` pseudo-property (the fold pass skips it).
    pub no_fold: bool,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct If {
    pub cond: Expr,
    pub then_block: Block,
    pub else_block: Option<Block>,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct OutBinding {
    pub name: String,
    pub value: Option<Expr>,
    pub typ: Option<TypeExpr>,
    pub side: Option<PortSide>,
    /// `@label("…")` display-text override for the port's floating label.
    pub label: Option<String>,
    /// `@label(expr)` — a compile-time-constant expression form of the
    /// above; folded to text at lowering. At most one of `label` /
    /// `label_expr` is ever set.
    pub label_expr: Option<Expr>,
    /// `@invisible`: the port's rerouter is emitted hidden.
    pub invisible: bool,
    /// `@nofold`: every IR node lowered from this declaration's subtree
    /// carries the `_nofold` pseudo-property (the fold pass skips it).
    pub no_fold: bool,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct ExprStmt {
    pub expr: Expr,
    pub range: SourceRange,
}

// ---------- expressions ----------

#[derive(Clone, Debug)]
pub enum Expr {
    IntLit {
        value: i64,
        text: String,
        range: SourceRange,
    },
    AtomLit {
        name: String,
        value: i64,
        range: SourceRange,
    },
    FloatLit {
        value: f64,
        text: String,
        range: SourceRange,
    },
    StringLit {
        value: String,
        range: SourceRange,
    },
    /// `"hello ${name}"` — parts alternate between literal fragments and
    /// embedded expressions.
    InterpLit {
        parts: Vec<InterpPart>,
        range: SourceRange,
    },
    BoolLit {
        value: bool,
        range: SourceRange,
    },
    Ident {
        name: String,
        range: SourceRange,
    },
    FieldAccess {
        obj: Box<Expr>,
        field: String,
        range: SourceRange,
    },
    IndexAccess {
        obj: Box<Expr>,
        index: Box<Expr>,
        range: SourceRange,
    },
    TuplePick {
        obj: Box<Expr>,
        index: usize,
        range: SourceRange,
    },
    UnOp {
        op: String,
        operand: Box<Expr>,
        range: SourceRange,
    },
    BinOp {
        op: String,
        left: Box<Expr>,
        right: Box<Expr>,
        range: SourceRange,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<CallArg>,
        /// Explicit type arguments: `pick<int>(...)`. Empty for an
        /// ordinary call (type params, if any, are inferred from the args).
        type_args: Vec<TypeExpr>,
        range: SourceRange,
    },
    Deref {
        operand: Box<Expr>,
        range: SourceRange,
    },
    RefOf {
        operand: Box<Expr>,
        range: SourceRange,
    },
    IfExpr {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
        range: SourceRange,
    },
    BlockExpr {
        stmts: Vec<Stmt>,
        value: Box<Expr>,
        range: SourceRange,
    },
    MatchExpr {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        range: SourceRange,
    },
    RecordLit {
        fields: Vec<RecordLitField>,
        range: SourceRange,
    },
    /// Array literal `[a, b, c, ...spread]`. Valid as a constant `array`/`var`
    /// initializer (all-literal elements, baked at load) or as an exec-context
    /// assignment value (desugars to clear + push/append).
    Array {
        elements: Vec<ArrayElem>,
        range: SourceRange,
    },
    /// Asset reference `$AssetType/AssetName` — an external asset the world
    /// embeds by name (weapon, audio/font descriptor, …).
    AssetRef {
        asset_type: String,
        asset_name: String,
        range: SourceRange,
    },
    /// Prefab file reference `$./rel/path.brz` (relative to the current source
    /// file) or `$/abs/path.brz` (filesystem-absolute). At emit the `.brz` is
    /// read, embedded via `World::add_prefab`, and the gate's `Prefab`
    /// bundle_path_ref property is set to the resulting `Prefabs/Uploads/…`
    /// path. `path` is the source-level string after `$` (e.g. `./turret.brz`).
    PrefabRef {
        path: String,
        range: SourceRange,
    },
    /// Inline nested-prefab block `` $```…``` `` — the enclosed text is a
    /// whole Wirescript source compiled as its own program and embedded as a
    /// prefab, the same way `PrefabRef` embeds a `.brz` on disk. `source` is
    /// the verbatim text between the fences (captured by the lexer).
    NestedPrefab {
        source: String,
        range: SourceRange,
    },
    /// Map literal `{ k => v, ... }` / `{ "k": v }` / `{ [expr]: v }`. Valid as
    /// a constant `map` initializer (all-literal entries, baked at load).
    MapLit {
        entries: Vec<MapLitEntry>,
        range: SourceRange,
    },
}

/// An element of an array literal: a single value or a `...spread` of another
/// array whose elements are appended in place.
#[derive(Clone, Debug)]
pub enum ArrayElem {
    Item(Expr),
    Spread(Expr),
}

impl ArrayElem {
    /// The inner expression, regardless of whether it's an item or a spread.
    pub fn expr(&self) -> &Expr {
        match self {
            ArrayElem::Item(e) | ArrayElem::Spread(e) => e,
        }
    }
    pub fn expr_mut(&mut self) -> &mut Expr {
        match self {
            ArrayElem::Item(e) | ArrayElem::Spread(e) => e,
        }
    }
    pub fn range(&self) -> &SourceRange {
        self.expr().range()
    }
}

impl Expr {
    pub fn range(&self) -> &SourceRange {
        match self {
            Expr::IntLit { range, .. }
            | Expr::AtomLit { range, .. }
            | Expr::FloatLit { range, .. }
            | Expr::StringLit { range, .. }
            | Expr::InterpLit { range, .. }
            | Expr::BoolLit { range, .. }
            | Expr::Ident { range, .. }
            | Expr::FieldAccess { range, .. }
            | Expr::IndexAccess { range, .. }
            | Expr::TuplePick { range, .. }
            | Expr::UnOp { range, .. }
            | Expr::BinOp { range, .. }
            | Expr::Call { range, .. }
            | Expr::Deref { range, .. }
            | Expr::RefOf { range, .. }
            | Expr::IfExpr { range, .. }
            | Expr::BlockExpr { range, .. }
            | Expr::MatchExpr { range, .. }
            | Expr::RecordLit { range, .. }
            | Expr::Array { range, .. }
            | Expr::AssetRef { range, .. }
            | Expr::PrefabRef { range, .. }
            | Expr::NestedPrefab { range, .. }
            | Expr::MapLit { range, .. } => range,
        }
    }

    pub fn range_mut(&mut self) -> &mut SourceRange {
        match self {
            Expr::IntLit { range, .. }
            | Expr::AtomLit { range, .. }
            | Expr::FloatLit { range, .. }
            | Expr::StringLit { range, .. }
            | Expr::InterpLit { range, .. }
            | Expr::BoolLit { range, .. }
            | Expr::Ident { range, .. }
            | Expr::FieldAccess { range, .. }
            | Expr::IndexAccess { range, .. }
            | Expr::TuplePick { range, .. }
            | Expr::UnOp { range, .. }
            | Expr::BinOp { range, .. }
            | Expr::Call { range, .. }
            | Expr::Deref { range, .. }
            | Expr::RefOf { range, .. }
            | Expr::IfExpr { range, .. }
            | Expr::BlockExpr { range, .. }
            | Expr::MatchExpr { range, .. }
            | Expr::RecordLit { range, .. }
            | Expr::Array { range, .. }
            | Expr::AssetRef { range, .. }
            | Expr::PrefabRef { range, .. }
            | Expr::NestedPrefab { range, .. }
            | Expr::MapLit { range, .. } => range,
        }
    }
}

#[derive(Clone, Debug)]
pub enum InterpPart {
    Lit(String),
    Expr(Box<Expr>),
}

#[derive(Clone, Debug)]
pub enum RecordLitField {
    Named {
        name: String,
        value: Expr,
        range: SourceRange,
    },
    Shorthand {
        name: String,
        range: SourceRange,
    },
    Spread {
        value: Expr,
        range: SourceRange,
    },
}

/// One `key => value` / `key: value` entry of a map literal.
#[derive(Clone, Debug)]
pub struct MapLitEntry {
    pub key: Expr,
    pub value: Expr,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub enum CallArg {
    Positional(Expr),
    Named { name: String, value: Expr },
    Spread(Expr),
}

#[derive(Clone, Debug)]
pub struct MatchArm {
    pub event_name: String,
    pub binding: Option<String>,
    pub body: MatchBody,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub enum MatchBody {
    Expr(Expr),
    Block(Block),
}
