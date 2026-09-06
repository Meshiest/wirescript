//! Compiling the same source twice produces the same bytes.
//!
//! Node ids used to come from a process-global counter, so a second compile in
//! the same process numbered its nodes differently, and through them ordered
//! bricks differently and produced a different byte length. Fresh processes
//! agreed with each other, which is what made it survive: nothing compiled the
//! same program twice in one process except a test harness, and no test
//! compared two compilations.
//!
//! This is the test that could not be written before, and it is the reason the
//! "never byte-diff compiler output" rule existed.

use wirescript::{CompileInput, FoldMode, compile};

fn brz(src: &str) -> Vec<u8> {
    compile(CompileInput {
        source: src,
        file: "determinism.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("program should compile")
    .brz
}

/// Programs chosen to exercise the id-minting paths that are not the plain
/// `add_node` one: chip instantiation (which re-mints a template's ids),
/// the fold pass's literal nodes, and boundary pins across a chip wall.
const PROGRAMS: &[&str] = &[
    "in go: exec\nvar n: int\non go { n = n + 1 }\n",
    "chip Add(a: int, b: int) -> (r: int) { out r = a + b }\n\
     in go: exec\nvar n: int\non go { let s = Add(1, 2)\n  n = s }\n",
    "var counter: int = 0\nvar log: int[]\nmod step() { counter = counter + 1\n  log.push(counter) }\n\
     in tick: exec\non tick { step()\n  step() }\n",
    "let A = 5\nlet B = A + 1\nvar m: int[] = [A, B, A * B]\nin go: exec\non go { }\n",
];

/// Many instances of one chip.
///
/// The sensitive path: instantiating a chip re-mints every node id of its
/// cached template, so where the ids start decides how the instances' nodes
/// hash, and layout walks `Module::nodes` (a hash map keyed by node id) to
/// place bricks. A flat program of the same size does not shift; this shape
/// does, which is why it is the one worth pinning.
fn many_chip_instances(n: usize) -> String {
    let mut s = String::from("chip Add(a: int, b: int) -> (r: int) { out r = a + b }\nin go: exec\n");
    for i in 0..n {
        s += &format!("var v{i}: int\n");
    }
    s += "on go {\n";
    for i in 0..n {
        s += &format!("  let s{i} = Add({i}, 1)\n  v{i} = s{i}\n");
    }
    s + "}\n"
}

#[test]
fn many_chip_instances_compile_the_same_way_twice() {
    let src = many_chip_instances(120);
    let first = brz(&src);
    for round in 1..4 {
        let again = brz(&src);
        assert_eq!(first.len(), again.len(), "round {round} changed the output size");
        assert!(again == first, "round {round} changed the output bytes");
    }
}

#[test]
fn repeated_compiles_in_one_process_are_byte_identical() {
    for src in PROGRAMS {
        let first = brz(src);
        for round in 1..4 {
            let again = brz(src);
            assert_eq!(
                first.len(),
                again.len(),
                "round {round} changed the output size for:\n{src}"
            );
            assert!(again == first, "round {round} changed the output bytes for:\n{src}");
        }
    }
}

/// Two DIFFERENT programs compiled in between must not disturb the third's
/// numbering either: the reset is per compile, not per process.
#[test]
fn an_interleaved_compile_does_not_change_the_next_one() {
    let target = PROGRAMS[1];
    let alone = brz(target);
    for other in PROGRAMS {
        let _ = brz(other);
        assert!(
            brz(target) == alone,
            "compiling another program first changed the output of:\n{target}"
        );
    }
}
