//!
//! Every node carries a [`SourceRange`] so diagnostics can attribute
//! errors back to source. The typechecker attaches resolved types later
//! by producing a parallel typed module — Phase 4 work.

use crate::diagnostic::SourceRange;

#[derive(Clone, Debug, Default)]
pub struct Script {
    pub decls: Vec<TopDecl>,
    pub range: SourceRange,
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
    pub typ: TypeExpr,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct VarDecl {
    pub name: String,
    pub typ: Option<TypeExpr>,
    pub init: Option<Expr>,
    pub is_static: bool,
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
    pub inputs: Vec<Param>,
    pub outputs: Vec<NamedOutput>,
    pub body: Block,
    pub range: SourceRange,
    /// When true, always expanded inline at call sites (no physical
    /// microchip). Set by the `mod` keyword.
    pub inline: bool,
}

/// Anonymous chip: `chip { body }` — shares parent scope, creates a
/// physical microchip grid for visual organization. Can have `in`/`out`
/// declarations inside the body for explicit I/O ports.
#[derive(Clone, Debug)]
pub struct AnonChipDecl {
    pub open: bool,
    pub body: Block,
    pub range: SourceRange,
}

/// `event foo = Trigger` or `event foo = on Trigger { ... }`
#[derive(Clone, Debug)]
pub struct EventDecl {
    pub name: String,
    pub source: Expr,
    pub captured_body: Option<Block>,
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
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct LetDecl {
    pub binding: LetBinding,
    pub typ: Option<TypeExpr>,
    pub value: Expr,
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
pub struct Handler {
    pub trigger: Trigger,
    /// `on Event(a, b) { ... }` — identifier params that bind the event's
    /// data outputs (e.g. `controller`, `arguments`).
    pub params: Vec<String>,
    /// Literal/named config args that configure the event gate itself, e.g.
    /// `on ChatCommand("greet", Description = "Greets you") { ... }`. These map
    /// to the gate's data-struct fields (not output bindings).
    pub config: Vec<HandlerConfigArg>,
    pub body: Block,
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
    Handler(Handler),
    AnonChip(AnonChipDecl),
    ChipDecl(ChipDecl),
    Return {
        value: Option<Expr>,
        range: SourceRange,
    },
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
            | Expr::PrefabRef { range, .. } => range,
        }
    }

    pub fn range_mut(&mut self) -> &mut SourceRange {
        match self {
            Expr::IntLit { range, .. }
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
            | Expr::PrefabRef { range, .. } => range,
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
