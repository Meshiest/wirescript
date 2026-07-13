//! Import-shape regressions.

use super::*;
use crate::ir::NodeKind;
use std::collections::HashSet;

fn is_pure(gate_class: &str) -> bool {
    gate_class == "_Literal" || gate_class.starts_with("BrickComponentType_WireGraph_Expr_")
}

/// Node ids that appear in any wire, as source or target.
fn wired_nodes(r: &LowerResult) -> HashSet<crate::ir::NodeId> {
    let mut set = HashSet::new();
    fn walk(m: &crate::ir::Module, set: &mut HashSet<crate::ir::NodeId>) {
        for w in &m.wires {
            set.insert(w.source.node_id);
            set.insert(w.target.node_id);
        }
        for c in m.chips.values() {
            walk(c, set);
        }
    }
    walk(&r.module, &mut set);
    set
}

fn orphan_pure_gates(r: &LowerResult) -> Vec<&'static str> {
    let wired = wired_nodes(r);
    fn walk<'a>(
        m: &'a crate::ir::Module,
        wired: &HashSet<crate::ir::NodeId>,
        out: &mut Vec<&'static str>,
    ) {
        for n in m.nodes.values() {
            if n.kind == NodeKind::Gate && is_pure(n.gate_class) && !wired.contains(&n.id) {
                out.push(n.gate_class);
            }
        }
        for c in m.chips.values() {
            walk(c, wired, out);
        }
    }
    let mut out = Vec::new();
    walk(&r.module, &wired, &mut out);
    out
}

/// A module imported via BOTH a namespace (`import * as x`) AND a named import
/// materializes its top-level `let`s twice — the namespace copy is unreferenced,
/// so a constant used to ship as a gate wired to nothing. `prune_dead_pure_gates`
/// must drop that orphan.
#[test]
fn namespace_plus_named_import_leaves_no_orphan_constant() {
    let lib = "\
let PAD = \"xxxxxxxx\"
mod pick(n: int) -> string {
  return PAD.Substring(0, n)
}
mod greet() -> string {
  return \"hi\"
}";
    let main = "\
import * as lib from \"lib\"
import { pick } from \"lib\"
in n: int
out r = pick(n)
out g = lib.greet()";
    let r = compile_multi(main, &[("lib", lib)]);
    assert_no_errors(&r);
    let orphans = orphan_pure_gates(&r);
    assert!(
        orphans.is_empty(),
        "double-import (namespace + named) left orphaned pure gate(s): {orphans:?}"
    );
}

/// A user's connected-but-unused pure computation is NOT a compiler-generated
/// orphan and must survive (the prune only removes fully-disconnected gates).
#[test]
fn unused_let_computation_is_kept() {
    let r = compile(
        "\
var x: int = 5
in player: character
on player { let y = x * 2 + 1 }",
    );
    assert_no_errors(&r);
    let has = |cls: &str| r.module.nodes.values().any(|n| n.gate_class == cls);
    assert!(
        has("BrickComponentType_WireGraph_Expr_MathMultiply"),
        "unused `let y = x * 2 + 1` should keep its MathMultiply (not DCE'd)"
    );
}
