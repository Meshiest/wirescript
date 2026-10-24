//! A whole program as the backend takes it: the root [`Module`] plus the
//! module-level settings that decide how it is laid out and emitted.
//!
//! Everything [`crate::emit::assemble`] reads is here, so a producer other
//! than the Wirescript front end can build one and get a save out of it.

use std::sync::Arc;

use super::{Literal, Module, Node};

/// A lowered program, ready to lay out and emit.
#[derive(Clone, Debug)]
pub struct Program {
    pub module: Module,
    pub settings: ProgramSettings,
}

impl Program {
    pub fn new(module: Module) -> Self {
        Self {
            module,
            settings: ProgramSettings::default(),
        }
    }
}

/// Module-level settings. Wirescript sets them from file annotations
/// (`@layout`, `@invisible`) and the module doc comment; every field defaults
/// to the plain case.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProgramSettings {
    /// The entry source file, as the compiler was given it. Informational:
    /// `assemble` names the save after it.
    pub source_file: Option<String>,
    /// Which engine lays out module interiors.
    pub layout: LayoutMode,
    /// The emitted top-level microchip shell is hidden, non-colliding, and
    /// carries no labels (root name, root plane header, var tags, I/O gate
    /// labels).
    pub invisible: bool,
    /// Drop the per-gate name labels and var tags. Wirescript sets it for
    /// `@layout("cube")`: a cube is a solid block of gates, so those labels
    /// cannot be read and are pure payload, one text component per labelled
    /// gate. The shell label and the plane header stay, as does a runtime
    /// `@label(expr)`, whose component is a live wire target rather than
    /// decoration.
    pub no_gate_labels: bool,
    /// The program was compiled with constant folding off (`--no-fold`).
    /// Emit does not read it; `assemble` compiles the program's `.ws` source
    /// prefabs the same way, so they come out as they did in the compile.
    pub no_fold: bool,
    /// Doc text rendered under the root plane's title.
    pub module_doc: Option<String>,
    /// The entry file's indentation and comments. Only [`LayoutMode::Code`]
    /// reads it; without it, code layout places rows by each node's own
    /// source column and draws no comment labels.
    pub source_map: Option<Arc<SourceMap>>,
}

/// Which placement engine lays out module interiors.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum LayoutMode {
    /// Flat topological DAG layout (the long-standing default). Modules past
    /// `layout::GRID_LAYOUT_THRESHOLD` nodes fall back to [`Self::Cube`],
    /// whose cost does not grow with the wire graph.
    #[default]
    Dag,
    /// Source-shaped layout: row = source line, indent = source column.
    Code,
    /// Compact 3D grid — nodes packed into a cube by brick size, skipping DAG
    /// analysis entirely. Reached either by size or by `@layout("cube")`.
    Cube,
}

/// Lexical facts about one source file that survive parsing: how far each
/// line is indented, and where its `//` comments are. The code-shaped
/// layout reproduces the source's own shape, so it needs the text's
/// geometry rather than the AST's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceMap {
    /// The file this map describes, spelled the way
    /// [`crate::diagnostic::SourceRange::file`] spells it. Every line number
    /// below is in this file's numbering, so a consumer holding nodes from
    /// another file must compare `file` before indexing anything here.
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
#[derive(Clone, Debug, PartialEq)]
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

/// Node property naming the storage gate or `fn` shell a handle var points
/// at, as that node's id. Emit bakes it into a reference once bricks have
/// their addresses.
pub const HANDLE_TARGET: &str = "_handle_target";

/// Node property on an `ArrayVar` or `MapVar` whose initializer is refs: the
/// storage gate each element points at, as node ids. An array's is a list of
/// ids; a map's is a map from each constant key to an id. Emit bakes each one
/// into a reference, as for [`HANDLE_TARGET`].
pub const HANDLE_TARGETS: &str = "_handle_targets";

/// Node property marking the var that holds the program's own microchip
/// brick (`GetMicrochipBrick()`); emit bakes it with a reference to that
/// brick.
pub const MICROCHIP_BRICK: &str = "_microchip_brick";

impl Program {
    /// The first difference between two programs, or `None` when they are
    /// the same IR: settings, and per module the nodes (every field), wires
    /// in order, inputs, outputs, scopes, labels, styles, value
    /// names and chips. `template_key` and `scope_captures` are not
    /// compared; only lowering reads them.
    pub fn diff(&self, other: &Program) -> Option<String> {
        if self.settings != other.settings {
            return Some(format!(
                "settings differ: {:?} vs {:?}",
                self.settings, other.settings
            ));
        }
        diff_module(&self.module, &other.module, "root")
    }
}

fn same_properties(
    a: &crate::collections::HashMap<crate::intern::Sym, Literal>,
    b: &crate::collections::HashMap<crate::intern::Sym, Literal>,
) -> bool {
    a.len() == b.len()
        && a.iter()
            .all(|(k, x)| b.get(k).is_some_and(|y| same_literal(x, y)))
}

/// Literal equality with floats compared by their bits: a NaN equals itself,
/// and `-0.0` differs from `0.0`, as both differ in a save.
fn same_literal(a: &Literal, b: &Literal) -> bool {
    let fs = |x: &[f64], y: &[f64]| x.iter().zip(y).all(|(p, q)| p.to_bits() == q.to_bits());
    match (a, b) {
        (Literal::Float(x), Literal::Float(y)) => x.to_bits() == y.to_bits(),
        (
            Literal::Vector { x, y, z },
            Literal::Vector {
                x: x2,
                y: y2,
                z: z2,
            },
        ) => fs(&[*x, *y, *z], &[*x2, *y2, *z2]),
        (
            Literal::Rotator { pitch, yaw, roll },
            Literal::Rotator {
                pitch: p2,
                yaw: y2,
                roll: r2,
            },
        ) => fs(&[*pitch, *yaw, *roll], &[*p2, *y2, *r2]),
        (
            Literal::Quat { x, y, z, w },
            Literal::Quat {
                x: x2,
                y: y2,
                z: z2,
                w: w2,
            },
        ) => fs(&[*x, *y, *z, *w], &[*x2, *y2, *z2, *w2]),
        (
            Literal::LinearColor { r, g, b, a },
            Literal::LinearColor {
                r: r2,
                g: g2,
                b: b2,
                a: a2,
            },
        ) => fs(&[*r, *g, *b, *a], &[*r2, *g2, *b2, *a2]),
        (Literal::Array(x), Literal::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_literal(p, q))
        }
        (Literal::Map(x), Literal::Map(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((k1, v1), (k2, v2))| same_literal(k1, k2) && same_literal(v1, v2))
        }
        (Literal::Record(x), Literal::Record(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((k1, v1), (k2, v2))| k1 == k2 && same_literal(v1, v2))
        }
        _ => a == b,
    }
}

fn diff_module(a: &Module, b: &Module, path: &str) -> Option<String> {
    use crate::intern::resolve;
    let here = |what: &str| Some(format!("{path}: {what}"));
    if a.name != b.name {
        return here(&format!(
            "name `{}` vs `{}`",
            resolve(a.name),
            resolve(b.name)
        ));
    }
    let mut ids: Vec<_> = a.nodes.keys().copied().collect();
    ids.sort();
    let mut other: Vec<_> = b.nodes.keys().copied().collect();
    other.sort();
    if ids != other {
        return here(&format!("node ids {ids:?} vs {other:?}"));
    }
    for id in &ids {
        let (x, y) = (&a.nodes[id], &b.nodes[id]);
        // Everything but the properties by `==`; the properties by their bits.
        let bare = |n: &Node| Node {
            properties: Default::default(),
            ..n.clone()
        };
        let same = same_properties(&x.properties, &y.properties) && bare(x) == bare(y);
        if !same {
            return here(&format!("{id} differs:\n  {x:?}\n  {y:?}"));
        }
    }
    if a.wires != b.wires {
        return here(&format!("wires {:?} vs {:?}", a.wires, b.wires));
    }
    if a.inputs != b.inputs || a.outputs != b.outputs {
        return here(&format!(
            "inputs/outputs {:?}/{:?} vs {:?}/{:?}",
            a.inputs, a.outputs, b.inputs, b.outputs
        ));
    }
    if a.scopes != b.scopes {
        return here("scopes differ");
    }
    if a.dynamic_labels != b.dynamic_labels
        || a.root_label_override != b.root_label_override
        || a.root_dynamic_label != b.root_dynamic_label
    {
        return here("labels differ");
    }
    if a.dynamic_colors != b.dynamic_colors
        || a.dynamic_materials != b.dynamic_materials
        || a.root_style != b.root_style
    {
        return here("styles differ");
    }
    if a.value_names != b.value_names {
        return here("value names differ");
    }
    let mut chips: Vec<_> = a.chips.keys().copied().collect();
    chips.sort();
    let mut other_chips: Vec<_> = b.chips.keys().copied().collect();
    other_chips.sort();
    if chips != other_chips {
        return here(&format!("chips {chips:?} vs {other_chips:?}"));
    }
    chips
        .iter()
        .find_map(|c| diff_module(&a.chips[c], &b.chips[c], &format!("{path}/{c}")))
}
