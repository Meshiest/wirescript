//! What the parser fills in when the text leaves something out, and the
//! naming tables both directions share. The writer omits exactly what these
//! reproduce, so a value here changes what the canonical form writes, never
//! what a program means.

use std::sync::LazyLock;

use crate::catalog::{ComponentKind, default_catalog};
use crate::collections::HashMap;
use crate::intern::{Sym, intern, resolve, sym};
use crate::ir::{GateIO, LayoutMode, Literal, NodeKind, PortSpec, ScopeKind, Type};

/// The prefix a class may be written without.
pub const CLASS_PREFIX: &str = "BrickComponentType_";

/// Class an `in` defaults to.
pub const INPUT_CLASS: &str = crate::ir::gate_class::MICROCHIP_INPUT;
/// Class an `out` defaults to.
pub const OUTPUT_CLASS: &str = crate::ir::gate_class::MICROCHIP_OUTPUT;
/// Class a `chip` defaults to.
pub const CHIP_CLASS: &str = crate::ir::gate_class::MICROCHIP_ALT;

/// Words the grammar gives a meaning where a class could stand.
pub const RESERVED: &[&str] = &["in", "out", "chip", "module", "loc", "to"];

/// The words that start a literal, as the parser reads and the writer
/// writes them.
pub mod word {
    pub const TRUE: &str = "true";
    pub const FALSE: &str = "false";
    pub const NULL: &str = "null";
    pub const INF: &str = "inf";
    pub const NAN: &str = "nan";
    pub const VEC: &str = "vec";
    pub const ROT: &str = "rot";
    pub const QUAT: &str = "quat";
    pub const LRGBA: &str = "lrgba";
    pub const MAP: &str = "map";
    pub const REC: &str = "rec";
    pub const ASSET: &str = "asset";
    pub const PREFAB: &str = "prefab";
    pub const NESTED_PREFAB: &str = "nested_prefab";
    pub const PREFAB_BYTES: &str = "prefab_bytes";
}

const LITERAL_WORDS: &[&str] = &[
    word::TRUE,
    word::FALSE,
    word::NULL,
    word::INF,
    word::NAN,
    word::VEC,
    word::ROT,
    word::QUAT,
    word::LRGBA,
    word::MAP,
    word::REC,
    word::ASSET,
    word::PREFAB,
    word::NESTED_PREFAB,
    word::PREFAB_BYTES,
];

/// Words that start a literal, so an attribute value spelled as one of them
/// is read as that literal rather than as a bare word.
pub fn is_literal_word(w: &str) -> bool {
    LITERAL_WORDS.contains(&w)
}

/// Whether `c` may start an identifier: `[A-Za-z_]`.
pub fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

/// Whether `c` may continue an identifier: `[A-Za-z0-9_]`.
pub fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Whether `s` is an identifier, so the text can spell it bare.
pub fn is_ident(s: &str) -> bool {
    s.bytes().next().is_some_and(is_ident_start) && s.bytes().all(is_ident_char)
}

/// How the text spells each layout engine (`layout = ...`).
pub const LAYOUT_MODES: &[(&str, LayoutMode)] = &[
    ("dag", LayoutMode::Dag),
    ("code", LayoutMode::Code),
    ("cube", LayoutMode::Cube),
];

/// How the text spells each node kind (`!kind ...`).
pub const NODE_KINDS: &[(&str, NodeKind)] = &[
    ("gate", NodeKind::Gate),
    ("event", NodeKind::Event),
    ("chip", NodeKind::Chip),
    ("input", NodeKind::Input),
    ("output", NodeKind::Output),
    ("coerce", NodeKind::Coerce),
];

/// One scope kind's spelling: its word, whether a quoted name follows it,
/// and the kind it builds from that name.
pub type ScopeKindWord = (&'static str, bool, fn(String) -> ScopeKind);

/// How the text spells each scope kind. `ChipBody` and `HandlerBody` carry a
/// quoted name; the others take none.
pub const SCOPE_KINDS: &[ScopeKindWord] = &[
    ("ModuleRoot", false, |_| ScopeKind::ModuleRoot),
    ("ChipBody", true, |name| ScopeKind::ChipBody { name }),
    ("HandlerBody", true, |trigger_label| {
        ScopeKind::HandlerBody { trigger_label }
    }),
    ("IfGroup", false, |_| ScopeKind::IfGroup),
    ("IfCond", false, |_| ScopeKind::IfCond),
    ("IfThen", false, |_| ScopeKind::IfThen),
    ("IfElse", false, |_| ScopeKind::IfElse),
    ("Block", false, |_| ScopeKind::Block),
];

/// The word a scope kind is written with, and the name it carries.
pub fn scope_kind_word(k: &ScopeKind) -> (&'static str, Option<&str>) {
    let tag = std::mem::discriminant(k);
    let (w, _, _) = SCOPE_KINDS
        .iter()
        .find(|(_, _, make)| std::mem::discriminant(&make(String::new())) == tag)
        .expect("every scope kind has a word");
    let name = match k {
        ScopeKind::ChipBody { name } => Some(name.as_str()),
        ScopeKind::HandlerBody { trigger_label } => Some(trigger_label.as_str()),
        _ => None,
    };
    (w, name)
}

/// How the text spells each type with no parameters.
pub static SCALAR_TYPES: &[(&str, Type)] = &[
    ("bool", Type::Bool),
    ("int", Type::Int),
    ("float", Type::Float),
    ("string", Type::String),
    ("vector", Type::Vector),
    ("rotator", Type::Rotator),
    ("quat", Type::Quat),
    ("color", Type::Color),
    ("entity", Type::Entity),
    ("character", Type::Character),
    ("controller", Type::Controller),
    ("brick", Type::Brick),
    ("zone", Type::Zone),
    ("teleport", Type::Teleport),
    ("prefab", Type::PrefabRef),
    ("exec", Type::Exec),
    ("any", Type::Any),
    ("opaque", Type::Opaque),
    ("never", Type::Never),
];

/// The words that start a type with parameters.
pub const COMPOUND_TYPE_WORDS: &[&str] = &[
    "map",
    "union",
    "tuple",
    "array",
    "outputs",
    "param",
    "enum",
    "component",
];

/// Whether `w` starts a type.
pub fn is_type_word(w: &str) -> bool {
    SCALAR_TYPES.iter().any(|(s, _)| *s == w) || COMPOUND_TYPE_WORDS.contains(&w)
}

struct ClassNames {
    /// Short spelling -> full class, for every class whose short spelling is
    /// unambiguous.
    short_to_full: HashMap<&'static str, &'static str>,
    /// Full class -> short spelling, same set.
    full_to_short: HashMap<&'static str, &'static str>,
}

static CLASS_NAMES: LazyLock<ClassNames> = LazyLock::new(|| {
    let full: Vec<&'static str> = default_catalog()
        .entries()
        .iter()
        .map(|g| resolve(intern(&g.component.class)))
        .collect();
    let is_full: crate::collections::HashSet<&str> = full.iter().copied().collect();
    let mut short_to_full = HashMap::default();
    let mut full_to_short = HashMap::default();
    for f in full {
        let Some(short) = f.strip_prefix(CLASS_PREFIX) else {
            continue;
        };
        if !is_ident(short) || is_full.contains(short) || RESERVED.contains(&short) {
            continue;
        }
        short_to_full.insert(short, f);
        full_to_short.insert(f, short);
    }
    ClassNames {
        short_to_full,
        full_to_short,
    }
});

/// How the writer spells `class`.
pub fn class_spelling(class: &'static str) -> &'static str {
    CLASS_NAMES
        .full_to_short
        .get(class)
        .copied()
        .unwrap_or(class)
}

/// The class a written name means: the short spelling's full class, or the
/// name itself.
pub fn resolve_class(written: &str) -> &'static str {
    match CLASS_NAMES.short_to_full.get(written) {
        Some(full) => full,
        None => resolve(intern(written)),
    }
}

/// Every short spelling with the class it means, for tests.
pub fn short_class_names() -> Vec<(&'static str, &'static str)> {
    let mut v: Vec<_> = CLASS_NAMES
        .short_to_full
        .iter()
        .map(|(s, f)| (*s, *f))
        .collect();
    v.sort();
    v
}

/// The kind a gate of `class` has unless it says otherwise.
pub fn default_kind(class: &str) -> NodeKind {
    match default_catalog().find_by_class(class) {
        Some(g) if g.component.kind == ComponentKind::Event => NodeKind::Event,
        _ => NodeKind::Gate,
    }
}

static CATALOG_PORTS: LazyLock<HashMap<&'static str, GateIO>> = LazyLock::new(|| {
    let mut out = HashMap::default();
    for g in default_catalog().entries() {
        let side = |ps: &[crate::catalog::Port]| {
            ps.iter()
                .map(|p| PortSpec::new(intern(&p.name), p.ty.to_type(&Type::Any, &Type::Exec)))
                .collect::<Vec<_>>()
        };
        out.insert(
            resolve(intern(&g.component.class)),
            GateIO {
                inputs: side(&g.component.inputs),
                outputs: side(&g.component.outputs),
            },
        );
    }
    out
});

/// Give the input `port` of `ports` the type `src` of the wire feeding it,
/// when that input is typed `any`. Whether it changed.
pub fn infer_any_input(ports: &mut GateIO, port: crate::intern::Sym, src: Type) -> bool {
    match ports
        .inputs
        .iter_mut()
        .find(|p| p.name == port && p.ty == Type::Any)
    {
        Some(p) => {
            p.ty = src;
            true
        }
        None => false,
    }
}

/// The type a literal value has on a wire.
pub fn literal_type(lit: &Literal) -> Type {
    match lit {
        Literal::Bool(_) => Type::Bool,
        Literal::Int(_) => Type::Int,
        Literal::Float(_) => Type::Float,
        Literal::String(_) => Type::String,
        Literal::Vector { .. } => Type::Vector,
        Literal::Rotator { .. } => Type::Rotator,
        Literal::Quat { .. } => Type::Quat,
        Literal::Color { .. } | Literal::LinearColor { .. } => Type::Color,
        Literal::PrefabRef { .. } | Literal::NestedPrefab { .. } | Literal::PrefabBytes(_) => {
            Type::PrefabRef
        }
        _ => Type::Any,
    }
}

/// The ports a gate written without a signature gets: the inventory's for an
/// inventory class, a literal's one output for `_Literal`, none otherwise.
pub fn default_ports(class: &str, value: Option<&Literal>) -> GateIO {
    if class == crate::ir::gate_class::LITERAL {
        return match value {
            Some(v) => GateIO::source(literal_type(v)),
            None => GateIO::default(),
        };
    }
    CATALOG_PORTS.get(class).cloned().unwrap_or_default()
}

/// The type of the output a reference names: `port`, or the only output.
pub fn output_type(ports: &GateIO, port: Option<&str>) -> Option<Type> {
    match port {
        Some(p) => ports
            .outputs
            .iter()
            .find(|o| resolve(o.name) == p)
            .map(|o| o.ty.clone()),
        None if ports.outputs.len() == 1 => Some(ports.outputs[0].ty.clone()),
        None => None,
    }
}

/// The name an anonymous chip's module gets: `_anon_n<id>`.
pub fn default_chip_module_name(id: u32) -> String {
    format!("_anon_n{id}")
}

/// How an annotation's value is spelled.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AnnValue {
    /// A bare key; the property is `true`.
    Flag,
    /// A string.
    Str,
    /// A bare word (`left`), stored as a string.
    Word,
    /// A node reference, stored as the node's id.
    Node,
    /// Node references: a list `[%a, %b]`, stored as a list of ids, or a map
    /// `map["k" => %a]` from constant keys, stored as a map to ids.
    Nodes,
}

/// Short attribute keys and the property each one spells.
pub static ANNOTATIONS: &[(&str, &LazyLock<Sym>, AnnValue)] = &[
    ("closed", &sym::CHIP_CLOSED, AnnValue::Flag),
    ("label", &sym::NAME_LABEL, AnnValue::Str),
    ("side", &sym::REROUTE_SIDE, AnnValue::Word),
    ("invisible", &sym::REROUTE_INVISIBLE, AnnValue::Flag),
    ("doc", &sym::DOC_TEXT, AnnValue::Str),
    ("handle_target", &sym::HANDLE_TARGET, AnnValue::Node),
    ("handle_targets", &sym::HANDLE_TARGETS, AnnValue::Nodes),
];

/// The flag that suppresses an input's or output's default `PortLabel`.
pub const NO_LABEL: &str = "nolabel";

pub fn annotation_by_key(key: &str) -> Option<(&'static str, AnnValue)> {
    ANNOTATIONS
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, p, v)| (resolve(***p), *v))
}

pub fn annotation_by_prop(prop: &str) -> Option<(&'static str, AnnValue)> {
    ANNOTATIONS
        .iter()
        .find(|(_, p, _)| resolve(***p) == prop)
        .map(|(k, _, v)| (*k, *v))
}
