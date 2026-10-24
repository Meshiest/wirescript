//! Tests for the scope-aware constant reads.

use super::*;

/// A scope whose values say what each binding is: 0 a non-value, 1 a `let`
/// (may be a constant), 2 any other value.
fn bound(v: &u8) -> Bound {
    match v {
        0 => Bound::NotAValue,
        1 => Bound::MayBeConstant,
        _ => Bound::Value,
    }
}

fn module(pairs: &[(&str, i64)], declared: &[&str]) -> ConstScope {
    ConstScope::new(
        Arc::new(
            pairs
                .iter()
                .map(|(n, v)| (n.to_string(), Literal::Int(*v)))
                .collect(),
        ),
        Arc::new(declared.iter().map(|n| n.to_string()).collect()),
        Arc::default(),
    )
}

fn push(scope: &mut Scope<u8>, consts: &mut ConstScope) {
    scope.push(crate::scope::ScopeTag::BLOCK);
    consts.push(scope.current_id());
}

fn pop(scope: &mut Scope<u8>, consts: &mut ConstScope) {
    scope.pop();
    consts.pop();
}

#[test]
fn a_binding_that_is_no_constant_hides_the_module_constant() {
    let mut consts = module(&[("K", 4)], &["K"]);
    let mut scope: Scope<u8> = Scope::new();
    assert_eq!(
        consts.get(&scope, &bound, "K", Admit::All),
        Some(&Literal::Int(4))
    );
    push(&mut scope, &mut consts);
    scope.insert("K", 2);
    assert_eq!(consts.get(&scope, &bound, "K", Admit::All), None);
    assert!(!consts.env(&scope, &bound, Admit::All).contains_key("K"));
    // A mod or type of the name hides nothing.
    push(&mut scope, &mut consts);
    scope.insert("K", 0);
    assert_eq!(
        consts.get(&scope, &bound, "K", Admit::All),
        None,
        "the value binding further out still hides it"
    );
    pop(&mut scope, &mut consts);
    pop(&mut scope, &mut consts);
    assert_eq!(
        consts.get(&scope, &bound, "K", Admit::All),
        Some(&Literal::Int(4)),
        "back once the scope closes"
    );
}

#[test]
fn a_let_of_a_constant_is_that_constant_in_its_own_frame_only() {
    let mut consts = module(&[("K", 4)], &["K"]);
    let mut scope: Scope<u8> = Scope::new();
    push(&mut scope, &mut consts);
    scope.insert("K", 1);
    consts.record("K", Literal::Int(9), Marks::default());
    assert_eq!(
        consts.get(&scope, &bound, "K", Admit::All),
        Some(&Literal::Int(9))
    );
    assert_eq!(
        consts.get(&scope, &bound, "K", Admit::Declared),
        None,
        "a plain `let` is not declared"
    );
    assert_eq!(
        consts.env(&scope, &bound, Admit::All).get("K"),
        Some(&Literal::Int(9))
    );
    // Bound again in the same frame to no constant: what the frame recorded is
    // forgotten.
    consts.forget("K");
    assert_eq!(consts.get(&scope, &bound, "K", Admit::All), None);
    // An inner binding that is no constant hides the frame's constant.
    consts.record("K", Literal::Int(9), Marks::default());
    push(&mut scope, &mut consts);
    scope.insert("K", 2);
    assert_eq!(consts.get(&scope, &bound, "K", Admit::All), None);
    assert!(!consts.env(&scope, &bound, Admit::All).contains_key("K"));
}

#[test]
fn a_value_binding_never_reads_a_constant_its_frame_recorded() {
    let mut consts = module(&[], &[]);
    let mut scope: Scope<u8> = Scope::new();
    push(&mut scope, &mut consts);
    consts.record("q", Literal::Int(5), Marks::default());
    scope.insert("q", 2);
    assert_eq!(consts.get(&scope, &bound, "q", Admit::All), None);
}

#[test]
fn a_constant_recorded_without_a_binding_answers_above_an_outer_binding() {
    let mut consts = module(&[], &[]);
    let mut scope: Scope<u8> = Scope::new();
    push(&mut scope, &mut consts);
    scope.insert("r", 2);
    push(&mut scope, &mut consts);
    consts.record(
        "r",
        Literal::Int(7),
        Marks {
            declared: true,
            ..Default::default()
        },
    );
    assert_eq!(
        consts.get(&scope, &bound, "r", Admit::Declared),
        Some(&Literal::Int(7))
    );
}

#[test]
fn placeholders_and_uncertified_values_answer_only_the_reads_that_admit_them() {
    let mut consts = module(&[], &[]);
    let mut scope: Scope<u8> = Scope::new();
    push(&mut scope, &mut consts);
    scope.insert("m", 1);
    scope.insert("b", 1);
    consts.record(
        "m",
        Literal::Int(0),
        Marks {
            declared: true,
            placeholder: true,
            uncertified: false,
        },
    );
    consts.record(
        "b",
        Literal::Int(i64::MIN),
        Marks {
            declared: true,
            placeholder: false,
            uncertified: true,
        },
    );
    for (admit, m, b) in [
        (Admit::All, true, true),
        (Admit::NoPlaceholders, false, true),
        (Admit::Declared, false, true),
        (Admit::Certified, true, false),
    ] {
        assert_eq!(consts.get(&scope, &bound, "m", admit).is_some(), m, "m");
        assert_eq!(consts.get(&scope, &bound, "b", admit).is_some(), b, "b");
        let env = consts.env(&scope, &bound, admit);
        assert_eq!((env.contains_key("m"), env.contains_key("b")), (m, b));
    }
}

#[test]
fn a_gate_holding_a_constant_container_reads_as_that_constant() {
    let list = Literal::Array(vec![Literal::Int(1)]);
    let mut consts = ConstScope::new(
        Arc::new([("T".to_string(), list.clone())].into_iter().collect()),
        Arc::default(),
        Arc::default(),
    );
    let mut scope: Scope<Literal> = Scope::new();
    let holds = |v: &Literal| Bound::Holds(v.clone());
    scope.push(crate::scope::ScopeTag::BLOCK);
    consts.push(scope.current_id());
    scope.insert("T", list.clone());
    assert_eq!(consts.get(&scope, &holds, "T", Admit::All), Some(&list));
    // A gate holding another value is not the constant of its name.
    scope.insert("T", Literal::Array(vec![Literal::Int(2)]));
    assert_eq!(consts.get(&scope, &holds, "T", Admit::All), None);
}

#[test]
fn a_value_binding_named_like_a_namespace_hides_its_members() {
    let mut consts = module(&[("K", 4), ("u.K", 4)], &["K"]);
    let mut scope: Scope<u8> = Scope::new();
    push(&mut scope, &mut consts);
    let env = consts.env(&scope, &bound, Admit::All);
    assert_eq!(env.get("u.K"), Some(&Literal::Int(4)));
    assert!(!env.binds("u"), "the namespace itself is no value");
    scope.insert("u", 2);
    let env = consts.env(&scope, &bound, Admit::All);
    assert!(env.binds("u"));
    assert_eq!(
        env.get("u.K"),
        None,
        "a local `u` hides the namespace's members"
    );
    assert_eq!(env.get("K"), Some(&Literal::Int(4)));
    pop(&mut scope, &mut consts);
    assert_eq!(
        consts.env(&scope, &bound, Admit::All).get("u.K"),
        Some(&Literal::Int(4)),
        "back once it closes"
    );
}

#[test]
fn a_binding_that_is_no_constant_is_bound_but_reads_as_nothing() {
    let mut consts = module(&[("K", 4)], &["K"]);
    let mut scope: Scope<u8> = Scope::new();
    push(&mut scope, &mut consts);
    scope.insert("K", 2);
    let env = consts.env(&scope, &bound, Admit::All);
    assert_eq!(env.get("K"), None);
    assert!(
        env.binds("K"),
        "an evaluator resolving `K` itself must leave it alone"
    );
    // A `let` of a constant recorded in a frame further in answers above it.
    push(&mut scope, &mut consts);
    scope.insert("K", 1);
    consts.record("K", Literal::Int(9), Marks::default());
    let env = consts.env(&scope, &bound, Admit::All);
    assert_eq!(env.get("K"), Some(&Literal::Int(9)));
}

#[test]
fn a_frame_change_answers_without_copying_the_module_table() {
    let pairs: Vec<(String, i64)> = (0..2000).map(|i| (format!("C{i}"), i)).collect();
    let refs: Vec<(&str, i64)> = pairs.iter().map(|(n, v)| (n.as_str(), *v)).collect();
    let mut consts = module(&refs, &[]);
    let mut scope: Scope<u8> = Scope::new();
    push(&mut scope, &mut consts);
    let first = consts.env(&scope, &bound, Admit::All);
    for i in 0..50 {
        scope.insert(&format!("C{i}"), 2);
        let env = consts.env(&scope, &bound, Admit::All);
        assert_eq!(env.get(&format!("C{i}")), None);
        assert_eq!(env.get("C1999"), Some(&Literal::Int(1999)));
        assert!(
            Arc::ptr_eq(&env.module, &first.module),
            "the module table is shared, not copied"
        );
    }
}
