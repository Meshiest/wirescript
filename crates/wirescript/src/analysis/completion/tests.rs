use super::*;
use crate::analysis::collect_symbols_for_file;
use crate::resolve::{FsLoader, resolve};
use crate::typecheck::enums::{EnumDef, EnumRegistry};

/// This build has no editor lookup, so no completed text names an enum here;
/// the completions that need one are tested through the LSP.
fn no_enum<'r>(_: &'r EnumRegistry, _: &str) -> Option<&'r EnumDef> {
    None
}

fn symbols_for(source: &str) -> Vec<SymbolDef> {
    let resolved = resolve(source, "test", &FsLoader);
    let tc = crate::typecheck::typecheck_with_inference(&resolved.ast, "test").0;
    collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("test"))
}

/// Marks the cursor in a row's source.
const CURSOR: char = '‸';

/// `src` without its cursor marker, and the marker's 0-based line and char
/// column.
fn at_cursor(src: &str) -> (String, usize, usize) {
    let (before, _) = src.split_once(CURSOR).expect("the source marks the cursor");
    let line = before.matches('\n').count();
    let col = before.rsplit('\n').next().unwrap_or("").chars().count();
    (src.replacen(CURSOR, "", 1), line, col)
}

fn labels_at(src: &str, prefabs: &[String]) -> Vec<String> {
    let (source, line, col) = at_cursor(src);
    completions(&source, &symbols_for(&source), line, col, prefabs, &no_enum)
        .into_iter()
        .map(|c| c.label)
        .collect()
}

/// Whether `label` matches `pat`: equal, or starting with it when `pat` ends
/// in `*`.
fn matches(label: &str, pat: &str) -> bool {
    match pat.strip_suffix('*') {
        Some(prefix) => label.starts_with(prefix),
        None => label == pat,
    }
}

/// One completion expectation: at the `‸` in `src`, every `has` pattern
/// matches some label and no `lacks` pattern matches any.
struct Row {
    label: &'static str,
    src: &'static str,
    has: &'static [&'static str],
    lacks: &'static [&'static str],
}

const fn row(
    label: &'static str,
    src: &'static str,
    has: &'static [&'static str],
    lacks: &'static [&'static str],
) -> Row {
    Row {
        label,
        src,
        has,
        lacks,
    }
}

#[track_caller]
fn check(rows: &[Row]) {
    check_with_prefabs(rows, &[]);
}

#[track_caller]
fn check_with_prefabs(rows: &[Row], prefabs: &[String]) {
    let mut failures = Vec::new();
    for r in rows {
        let got = labels_at(r.src, prefabs);
        let missing: Vec<&&str> = r
            .has
            .iter()
            .filter(|p| !got.iter().any(|l| matches(l, p)))
            .collect();
        let leaked: Vec<&&str> = r
            .lacks
            .iter()
            .filter(|p| got.iter().any(|l| matches(l, p)))
            .collect();
        if !missing.is_empty() || !leaked.is_empty() {
            failures.push(format!(
                "{}: missing {missing:?}, leaked {leaked:?}, got {got:?}",
                r.label
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// What an empty position, a `$` reference or a type annotation offers.
#[test]
fn the_global_list_and_reference_fragments_complete() {
    check(&[
        row(
            "the global list offers builtin events and functions",
            "‸",
            &["ChatCommand", "RoundStart", "CharacterSpawned", "GetAim"],
            &[],
        ),
        row(
            "the global list offers keywords and builtins",
            "‸",
            &["var", "import", "Random", "DisplayText"],
            &[],
        ),
        row(
            "the global list offers the file's symbols",
            "var score: int = 0\n‸",
            &["score"],
            &[],
        ),
        row(
            "annotations include @label and @closed",
            "‸",
            &["@left", "@label", "@closed"],
            &[],
        ),
        // The module-level run that opens a file is offered too: these are the
        // annotations a reader is least likely to know exist.
        row(
            "annotations include the module-level run",
            "‸",
            &["@fold", "@nofold", "@layout", "@flat"],
            &[],
        ),
        // After `$`: asset types, in an isolated context where keywords and
        // functions must not leak in.
        row(
            "`$` completes asset types",
            "let w = $‸",
            &["BRItemBase", "BrickAudioDescriptor"],
            &["if"],
        ),
        row(
            "`$Type/` completes that type's asset names",
            "let w = $BRItemBase/‸",
            &["Weapon_Pistol"],
            &["BRItemBase"],
        ),
        // `weapon = <here>` offers full `$BRItemBase/Name` refs (the author
        // needn't know the type name), gated on the param being
        // constant-only config.
        row(
            "an asset config param completes typed refs",
            "on go { GiveWeapon(c, weapon = ‸) }",
            &["$BRItemBase/Weapon_Pistol"],
            &[],
        ),
        // `font = <here>` resolves to the font descriptor type.
        row(
            "a font config param completes font refs",
            "in p: controller\non go { p.DisplayText(\"hi\", font = ‸) }",
            &["$BrickFontDescriptor/Roboto"],
            &[],
        ),
        // `var e: <here>` offers the built-in game enum type names, seeded
        // straight from the catalog: game enums never get a symbol.
        row(
            "a type annotation completes a built-in game enum name",
            "var e: Eas‸",
            &["EasingFunction"],
            &[],
        ),
    ]);
}

#[test]
fn prefab_ref_completes_from_candidate_paths() {
    let prefabs = ["./turret.brz", "./enemies/tank.brz", "./notes.txt"].map(String::from);
    // `SpawnPrefab(prefab = $./t` offers the `./t…` prefab paths; `./t`
    // doesn't match `./enemies/…`.
    check_with_prefabs(
        &[row(
            "`$./t` offers `./t…` paths",
            "on x { SpawnPrefab(prefab = $./t‸) }",
            &["./turret.brz"],
            &["./enemies/tank.brz"],
        )],
        &prefabs,
    );
}

/// Inside a call's parentheses: its parameter names, or, in a value slot,
/// what the slot takes.
#[test]
fn call_arguments_complete() {
    check(&[
        row(
            "SendCustomEvent offers its event name, not its target",
            "in go: exec\non go { SendCustomEvent(‸ }",
            &["eventName"],
            &["target*"],
        ),
        row(
            "SendGlobalCustomEvent offers its event name, not its target",
            "in go: exec\non go { SendGlobalCustomEvent(‸ }",
            &["eventName"],
            &["target*"],
        ),
        // `on Clock(<here>)` offers the event's wired input + config args (the
        // call-param path has no CallSpec for an event).
        row(
            "event config params",
            "on Clock(‸) { }",
            &["enabled = ", "interval = "],
            &[],
        ),
        // Every EBrickDirection member; the sentinel is never offered.
        row(
            "an enum value slot offers every member",
            "on go { SweepSimple(500.0, direction = ‸) }",
            &["X_Positive", "Y_Negative"],
            &["MAX"],
        ),
        // In a non-enum / non-asset value slot (`textId = <here>`), in-scope
        // identifiers, NOT the call's argument names.
        row(
            "a plain value slot offers in-scope idents, not arg names",
            "in c: controller\nlet myVal = 7\nin go: exec\non go {\n  c.DisplayText(\"hi\", textId = ‸)\n}",
            &["myVal"],
            &["positionX = "],
        ),
        // The gate's raw settings-menu field names are offered alongside
        // params.
        row(
            "data-driven config offers raw field names",
            "on go { SweepSimple(500.0, ‸) }",
            &["bOnlyHitPlayerBodyParts = ", "Direction = "],
            &[],
        ),
        // `Direction = <here>` (raw field) still offers the enum members.
        row(
            "a raw enum config value offers its members",
            "on go { SweepSimple(500.0, Direction = ‸) }",
            &["X_Positive"],
            &[],
        ),
        // A user mod's param names, not the whole global list.
        row(
            "a user mod call offers its param names",
            "mod shift(dist: int, fast: bool) {}\non x { shift(‸) }",
            &["dist", "fast"],
            &["if"],
        ),
        // `Vec(‸)`, all params required, offers x/y/z, not the global list.
        row(
            "an all-required builtin call offers its params",
            "let v = Vec(‸)",
            &["x", "y", "z"],
            &["if"],
        ),
        // The already-bound receiver param is not offered.
        row(
            "a method call skips its receiver param",
            "in ctrl: controller\non x { ctrl.DisplayText(\"hi\", ‸) }",
            &["fontSize*"],
            &["controller", "target"],
        ),
        // A plain `Call(<here>` (the dot belongs to the callee) still offers
        // the call's optional params: member access must not hijack it.
        row(
            "a plain call paren still shows params",
            "in pl: character\non x { pl.DisplayText(\"hi\", ‸) }",
            &["fontSize*"],
            &[],
        ),
        // `Easing`'s `function` param, backed by `EBREasingFunction`, offers
        // the qualified `EasingFunction.Bounce` form alongside the bare
        // `Bounce`, so an author can disambiguate at the call site the same
        // way they would construct the value directly.
        row(
            "a config arg value offers the qualified built-in enum variant alongside the bare name",
            "on go { let e = Easing(0.0, 1.0, 0.5, function = ‸) }\nin go: exec",
            &["Bounce", "EasingFunction.Bounce"],
            &[],
        ),
    ]);
}

/// After `receiver.`: only the receiver's members, never the global list.
#[test]
fn members_complete_after_a_dot() {
    check(&[
        // The global list must NOT leak into a member-access context.
        row(
            "a string offers only string methods",
            "let foo = \"\"\nfoo.‸",
            &["Length", "Contains"],
            &["if", "int", "GetAim", "ChatCommand"],
        ),
        // The full method set, not just push/pop/length.
        row(
            "an array offers its full method set",
            "var xs: int[]\nxs.‸",
            &["push", "find", "sort", "insert", "append", "slice"],
            &["if"],
        ),
        // An array-typed var completes array methods, not the var Value/prev
        // fields.
        row(
            "an array var offers array methods",
            "var ids: string[]\nids.‸",
            &["push", "find"],
            &[],
        ),
        // A file-scope `players: string` and a handler-local `players:
        // character[]`: inside the handler `players.` completes ARRAY methods
        // (the in-scope array); a flat name lookup would pick the first decl.
        row(
            "a shadowed var completes by scope, not by first decl",
            "var players: string = \"\"\non t {\n  var players: character[]\n  players.‸\n}",
            &["push", "fillFromPlayers"],
            &["Contains"],
        ),
        // The MAP method table: the map-only names and NOT array-only ones.
        row(
            "a map var offers map methods, not array ones",
            "var m: Map<string, int>\nm.‸",
            &["get", "set", "has", "remove", "clear", "keys", "values"],
            &["push", "find"],
        ),
        row(
            "a map through a type alias",
            "type Scores = Map<string, int>\nvar s: Scores\ns.‸",
            &["get", "has"],
            &["push"],
        ),
        // The generic-alias instance resolves by base name to the map table.
        row(
            "a map through a generic alias instance",
            "type Grid<T> = Map<string, T>\nvar g: Grid<int>\ng.‸",
            &["get", "set"],
            &["push"],
        ),
        // `.Value`/`.prev` AND the vector's methods and swizzle.
        row(
            "a typed var offers Value and its type's methods",
            "var pos: vector = Vec(1.0, 2.0, 3.0)\npos.‸",
            &["Value", "Normalize", "x"],
            &[],
        ),
        row(
            "a static var offers Value and prev",
            "static var score: int = 0\nscore.‸",
            &["Value", "prev"],
            &[],
        ),
        row(
            "a named record alias offers its fields",
            "type Pos = { x: int, y: int }\nlet p: Pos = { x: 1, y: 2 }\np.‸",
            &["x", "y"],
            &[],
        ),
        // An indexed read offers the read's outputs.
        row(
            "an indexed read offers its outputs",
            "var xs: int[]\nlet a = xs[0].‸",
            &["OutOfBounds"],
            &[],
        ),
        // `on split.<here>` where `split = pl.InputReader()` completes the
        // splitter's record fields; member context is isolated.
        row(
            "an input-reader field trigger offers its record fields",
            "in pl: character\nlet split = pl.InputReader()\non split.‸",
            &["Forward", "Right", "Up"],
            &["if"],
        ),
        // `pl.AddVelocity(linear = pl.<here>` completes pl's methods, not the
        // enclosing AddVelocity's params: the member access wins.
        row(
            "a receiver method completes inside a nested call arg",
            "in pl: character\non x { pl.AddVelocity(linear = pl.‸) }",
            &["GetVelocity", "GetLocation"],
            &["linear = "],
        ),
    ]);
}

#[test]
fn identifier_completion_dedups_shadowed_var_by_scope() {
    // A file-scope `players: string` and a handler-local `players:
    // character[]`. Completing a bare identifier at file scope (above and
    // outside the handler) must offer a SINGLE `players`, typed `string`:
    // the handler-local array is neither in scope nor declared yet.
    let src = "var players: string = \"\"\nplayers\non tick {\n  var players: character[]\n}";
    let items = completions(src, &symbols_for(src), 1, 7, &[], &no_enum);
    let players: Vec<&Completion> = items.iter().filter(|i| i.label == "players").collect();
    assert_eq!(players.len(), 1, "expected one `players`, got: {players:?}");
    assert_eq!(
        players[0].detail.as_deref(),
        Some("string"),
        "file-scope `players` must show its string type, not the handler-local array"
    );
}

#[test]
fn namespace_dot_completes_members() {
    // `import * as u` exposes qualified `u.member` symbols; `u.` lists the
    // members by their bare names, and they never leak into the global list.
    let symbols = vec![
        SymbolDef {
            name: "u".into(),
            kind: "namespace",
            range: Default::default(),
            ty: None,
            exec: false,
            is_const: false,
        },
        SymbolDef {
            name: "u.swap".into(),
            kind: "mod",
            range: Default::default(),
            ty: None,
            exec: false,
            is_const: false,
        },
        SymbolDef {
            name: "u.clamp".into(),
            kind: "fn",
            range: Default::default(),
            ty: None,
            exec: false,
            is_const: false,
        },
    ];
    let labels = |src: &str, col: usize| -> Vec<String> {
        completions(src, &symbols, 0, col, &[], &no_enum)
            .into_iter()
            .map(|i| i.label)
            .collect()
    };
    let src = "let a = u.";
    let ls = labels(src, src.len());
    assert!(ls.contains(&"swap".to_string()), "swap missing: {ls:?}");
    assert!(ls.contains(&"clamp".to_string()), "clamp missing: {ls:?}");
    assert!(
        !ls.iter().any(|l| l.contains('.')),
        "qualified name leaked: {ls:?}"
    );
    // A bare-identifier position must NOT list the qualified members.
    let bare = labels("", 0);
    assert!(
        bare.contains(&"u".to_string()),
        "namespace alias missing: {bare:?}"
    );
    assert!(
        !bare.iter().any(|l| l.contains('.')),
        "qualified leaked into global: {bare:?}"
    );
}

/// The match-arm completion scanner takes the cursor's char column and
/// slices `source` at it. It used to convert that column with a
/// byte-indexed helper, so with the cursor just past a multi-byte
/// character the slice landed mid-character and the completion request
/// panicked instead of answering.
#[test]
fn match_arm_scrutinee_scan_survives_a_cursor_after_a_multibyte_char() {
    let src = "enum Shape { Empty, Circle(float) }\nin s: Shape\nout x = match s {\n  Empty => \"\u{e9}\n}\n";
    let line = 3;
    // Cursor at the end of the line, i.e. immediately after the `é`.
    let col = src.lines().nth(line).unwrap().chars().count();
    assert!(
        match_arm_head_scrutinee_at(src, line, col).is_none(),
        "the cursor is in an arm BODY, not a pattern head"
    );
}

/// Completion lists the document's folder only inside a `$./` or `$/` file
/// reference, so ordinary typing never touches the disk.
#[test]
fn prefab_path_fragment_only_inside_a_file_reference() {
    fn at_end(s: &str) -> Option<(usize, &str)> {
        prefab_path_fragment(s, 0, s.chars().count())
    }
    assert_eq!(at_end("let pf = $./sub/a"), Some((9, "./sub/a")));
    assert_eq!(at_end("let pf = $/abs"), Some((9, "/abs")));
    for outside in [
        "",
        "let x = 1",
        "let c = $Component/Point",
        "let pf = $./a.brz + b",
        "on go {",
    ] {
        assert_eq!(at_end(outside), None, "{outside:?}");
    }
}
