//! Emit configuration and result types.

use super::*;

/// Grid-space position of a single IR node inside its containing chip
/// (or on the global grid for the outer microchip brick).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl From<Placement> for Position {
    fn from(p: Placement) -> Self {
        Position {
            x: p.x,
            y: p.y,
            z: p.z,
        }
    }
}

/// Resolves a prefab file reference (`$./file.brz` / `$/abs/file.brz`) to the
/// raw `.brz` bytes to embed. The argument is the source-level path (after the
/// `$`). Frontends supply this: the CLI reads from disk relative to the source
/// file; the wasm/playground sandbox looks up dragged-in files. `Err` carries a
/// human-readable reason (missing file, read error) surfaced as an emit error.
#[derive(Clone)]
pub struct PrefabResolver(
    pub std::sync::Arc<dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync>,
);

impl PrefabResolver {
    pub fn new(f: impl Fn(&str) -> Result<Vec<u8>, String> + Send + Sync + 'static) -> Self {
        PrefabResolver(std::sync::Arc::new(f))
    }
    pub(super) fn resolve(&self, path: &str) -> Result<Vec<u8>, String> {
        (self.0)(path)
    }
}

impl std::fmt::Debug for PrefabResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrefabResolver(..)")
    }
}

/// Compiles an inline nested-prefab block (`$``` ... ``` `) to `.brz` bytes to
/// embed, mirroring [`PrefabResolver`]. The argument is the inner source text
/// and the current nesting depth (1 for a block written directly in the root
/// source); `Err` carries a human-readable reason surfaced as an emit error.
#[derive(Clone)]
pub struct NestedCompiler(
    pub std::sync::Arc<dyn Fn(&str, usize) -> Result<Vec<u8>, String> + Send + Sync>,
);

impl NestedCompiler {
    pub fn new(f: impl Fn(&str, usize) -> Result<Vec<u8>, String> + Send + Sync + 'static) -> Self {
        NestedCompiler(std::sync::Arc::new(f))
    }
    pub(super) fn compile(&self, src: &str, depth: usize) -> Result<Vec<u8>, String> {
        (self.0)(src, depth)
    }
}

impl std::fmt::Debug for NestedCompiler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NestedCompiler(..)")
    }
}

/// Options for a single emit run.
#[derive(Clone, Debug)]
pub struct EmitOptions {
    /// World position of the outer deployment chip brick, in global-grid units.
    pub chip_pos: Placement,
    /// Bundle description written to the .brz metadata.
    pub description: String,
    /// When true, the root microchip is emitted uncollapsed (expanded).
    /// Non-root chips are always open unless annotated `@closed`.
    pub open: bool,
    /// Resolves `$./file.brz` / `$/abs/file.brz` prefab references to bytes.
    /// `None` makes any prefab reference an emit error.
    pub prefab_resolver: Option<PrefabResolver>,
    /// Compiles inline nested-prefab blocks (`$``` ... ``` `) to bytes.
    /// `None` makes any nested-prefab block an emit error.
    pub nested_compiler: Option<NestedCompiler>,
    /// Doc comment rendered under the root plane's title (module-level `///`
    /// block — the doc attached to the file's first declaration, mirroring
    /// how namespace imports derive their module doc).
    pub module_doc: Option<String>,
    /// Module-level `@invisible` — the emitted top-level microchip shell is
    /// hidden, non-colliding, and carries no labels (root name, root plane
    /// header, var tags, I/O gate labels).
    pub invisible: bool,
}

impl Default for EmitOptions {
    fn default() -> Self {
        Self {
            chip_pos: Placement { x: 0, y: 0, z: 0 },
            description: String::from("wirescript emit"),
            open: false,
            prefab_resolver: None,
            nested_compiler: None,
            module_doc: None,
            invisible: false,
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum EmitError {
    #[error("node {0} has no placement")]
    MissingPlacement(String),
    #[error("wire references unknown node: {0}")]
    UnknownWireNode(String),
    #[error("brdb error: {0}")]
    Brdb(#[from] brdb::BrError),
    #[error("prefab reference `${0}`: {1}")]
    PrefabResolve(String, String),
    /// A gutter-bus wire whose endpoint could not be resolved to a brick port.
    ///
    /// Fatal, unlike the module-wire equivalent: the bus SUPPRESSED the module
    /// wire this one replaces, so there is no second path to the consumer. A
    /// dropped bus wire means the value never arrives, and nothing downstream
    /// can tell — the save loads, pastes, and reads zero.
    #[error("bus wire {from} → {to} could not be drawn: {cause}")]
    BusWireUnresolved {
        from: String,
        to: String,
        cause: String,
    },
    /// A module wire (pass 3) or a runtime-`@label` wire (pass 3.5) whose
    /// endpoint could not be resolved to a brick port.
    ///
    /// Fatal. A wire that can't be drawn is not a cosmetic gap — it is the
    /// signature of a lowering miscompile (a stranded `return`/`emit` fan-in, a
    /// template-cache mixup, a phantom node) that would otherwise slip into a
    /// format-valid `.brz` and silently misbehave in-game. This path used to log
    /// to stderr and continue, laundering every such bug into a shippable save;
    /// erroring here converts the whole class into a compile failure. (The
    /// gutter-bus equivalent is `BusWireUnresolved`.)
    #[error("dropped wire: {0}")]
    DroppedWire(String),
}
