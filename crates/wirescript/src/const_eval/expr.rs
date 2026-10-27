//! Compile-time expression evaluation.
//!
//! Tries `const_eval::expr_to_literal_in` first (literals, named constants,
//! operators and the `Vec`/`Rotation`/`Color` constructors), then evaluates
//! the forms it does not fold. A failure carries a reason, so the diagnostic
//! names the offending value.

use std::sync::Arc;

use crate::ast::{
    ArrayElem, CallArg, ChipDecl, Expr, ImportScope, InterpPart, MatchArm, MatchBody,
    RecordLitField,
};
use crate::collections::HashMap;
use crate::const_scope::ConstNames;
use crate::diagnostic::{SourceRange, StartKey};
use crate::ir::{Literal, Type};
use crate::lower::fold::eval::{self as fold_eval, Value};
use crate::lower::matchtree::{self, ConstOutcome};
use crate::scope::top::TopNames;
use crate::typecheck::enums::{DISC_FIELD, DISCRIMINANT_FIELD, EnumDef, EnumRegistry, slot_name};

use super::error::{ConstError, ConstReason};
use super::interp::Budget;
use super::{ConstEnv, eval_const_binop, eval_const_unop, expr_to_literal_in, fold_constructor};

// Two evaluators, two laws. `expr_to_literal_in` computes mixed-sign integer
// division and maps a non-finite result to 0; `fold::eval` refuses both, since
// in-game tests never certified them. String interpolation and method calls go
// through the certified `fold::eval`, whose refusals are WS047
// (`ConstReason::Refused`). Unifying them needs re-certification; the test
// `both_evaluators_agree_where_both_are_defined` guards the seam.

/// Call names folded as builtin constructors (by
/// `const_eval::fold_constructor`), never treated as mod calls, so a
/// failure is the constructor declining its arguments. Each must also be a
/// `catalog::calls` entry: [`bind_constructor_args`] binds through it.
/// `ColorSRGB` folds by `fold_srgb_color`, which const evaluation never reaches.
pub(super) const FOLDABLE_CONSTRUCTORS: &[&str] = &["Vec", "Rotation", "Color"];

/// Refusal messages shared by `eval_expr` and `reason_for`, so both paths word
/// the same cause the same way.
fn binop_refused(op: &str) -> String {
    format!("the `{op}` operator has no certified constant result for these operands")
}

fn unop_refused(op: &str) -> String {
    format!("the `{op}` operator has no certified constant result for this operand")
}

fn constructor_refused(name: &str) -> String {
    format!("`{name}` has no constant form for these arguments")
}

/// The constants of each module imported `import * as`, by the file
/// declaring it, as that module's own code reads them by name (see
/// [`ConstCtx::module_of`]).
pub(crate) type ModuleConsts = Arc<HashMap<Arc<str>, Arc<ConstEnv>>>;

/// What a `const mod` body reads when a file declares a name more than once
/// at its top level: `base`, with each such name of the `const mod`'s own file
/// holding the constant of the declaration a body declared where the `const
/// mod` is names (see [`crate::scope::top::TopNames::visible_decl`]), and
/// absent where that is no constant. A name another file declares more than
/// once is absent.
pub(crate) struct ModDups {
    base: Arc<ConstEnv>,
    top: Arc<TopNames>,
    values: Arc<crate::const_eval::DupConsts>,
    at: std::sync::Mutex<ByDecl>,
}

/// The constants of each `const mod` body by its declaration's file and
/// start, once built.
type ByDecl = HashMap<StartKey, Arc<ConstEnv>>;

impl ModDups {
    pub(crate) fn new(
        base: Arc<ConstEnv>,
        top: Arc<TopNames>,
        values: Arc<crate::const_eval::DupConsts>,
    ) -> Self {
        Self {
            base,
            top,
            values,
            at: std::sync::Mutex::default(),
        }
    }

    /// The constants the body of a `const mod` declared at `decl` reads.
    fn of(&self, decl: &SourceRange) -> Arc<ConstEnv> {
        let key = decl.start_key();
        let mut at = self
            .at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(env) = at.get(&key) {
            return Arc::clone(env);
        }
        let mut out = (*self.base).clone();
        let read = crate::scope::top::ReadAt::body(decl);
        for (name, decls) in self.top.dups() {
            let own = decls[0].range.file == decl.file;
            let seen = if own {
                self.top.visible_decl(name, &read).flatten()
            } else {
                None
            };
            match seen.and_then(|d| self.values.get(&crate::const_eval::dup_key(d, name))) {
                Some(lit) => out.insert(name.to_string(), lit.clone()),
                None => out.remove(name),
            };
        }
        let env = Arc::new(out);
        at.insert(key, Arc::clone(&env));
        env
    }
}

/// Everything const evaluation needs to resolve an expression.
pub(crate) struct ConstCtx<'a> {
    /// Every constant visible at the current point (see
    /// [`crate::const_scope::ConstNames`]). Names resolve against this, and an
    /// enum variant, enum or namespace name is shadowed by a binding here.
    pub(super) consts: ConstNames,
    /// The module-level constants only, with no scope-local frames. A `const
    /// mod` body evaluates against these plus its parameters, so it never sees
    /// the caller's local bindings.
    pub(super) module_consts: Arc<ConstEnv>,
    /// The constants of each `import * as` module, by its file: what a
    /// `const mod` declared in that module evaluates against in place of
    /// `module_consts`, so its body reads its own module's constants, never
    /// the importer's of the same name.
    pub(super) module_by_file: ModuleConsts,
    /// Where a file declares a name more than once at its top level, what
    /// the body of each `const mod` reads in place of `module_consts`.
    pub(super) mod_dups: Option<Arc<ModDups>>,
    /// Resolves a callee name to its mod declaration, so a call can be
    /// checked for being a `const mod`. With `None` every call is "not found",
    /// as for an undeclared name. Each call site builds its own short-lived
    /// closure, since a `&self` method cannot return one that borrows `self`.
    pub lookup_mod: Option<&'a dyn Fn(&str) -> Option<Arc<ChipDecl>>>,
    /// Every declared `enum` by name, for `Enum.Variant` and `.Discriminant`.
    /// Shared by `Arc` rather than borrowed, so the struct needs no second
    /// lifetime.
    pub enum_defs: Arc<EnumRegistry>,
    /// Which imported declarations each file named: a `const mod` is called
    /// only where the calling file can name it, as at run time (see
    /// [`crate::ast::ImportScope::user_mod_answers`]).
    pub imports: Arc<ImportScope>,
    /// The program's type declarations, which a constant with a declared type
    /// is held in (see [`super::coerce_const_to_type`]).
    pub(crate) types: Arc<super::DeclaredTypes>,
    /// What each type parameter of the `const mod` body being evaluated
    /// stands for (see `interp::eval_call`): a declared type inside the body
    /// reads a parameter through this, never an alias of the same name.
    pub(super) type_args: crate::types::infer::Subst,
}

impl<'a> ConstCtx<'a> {
    /// An evaluation reading names through `consts`, calling a `const mod`
    /// against `module_consts` (or, for one declared in an `import * as`
    /// module, against that module's own constants in `module_by_file`).
    pub(crate) fn new(
        consts: ConstNames,
        module_consts: Arc<ConstEnv>,
        module_by_file: ModuleConsts,
        enum_defs: Arc<EnumRegistry>,
        imports: Arc<ImportScope>,
        types: Arc<super::DeclaredTypes>,
        lookup_mod: Option<&'a dyn Fn(&str) -> Option<Arc<ChipDecl>>>,
    ) -> Self {
        Self {
            consts,
            module_consts,
            module_by_file,
            mod_dups: None,
            lookup_mod,
            enum_defs,
            imports,
            types,
            type_args: crate::types::infer::Subst::default(),
        }
    }

    /// This evaluation, its `const mod` bodies reading a name declared more
    /// than once at the top level by position (see [`ModDups`]).
    pub(crate) fn with_mod_dups(mut self, mod_dups: Option<Arc<ModDups>>) -> Self {
        self.mod_dups = mod_dups;
        self
    }

    /// A copy reading names through `consts` instead.
    pub(super) fn with_consts(&self, consts: ConstNames) -> Self {
        Self {
            consts,
            module_consts: Arc::clone(&self.module_consts),
            module_by_file: Arc::clone(&self.module_by_file),
            mod_dups: self.mod_dups.clone(),
            lookup_mod: self.lookup_mod,
            enum_defs: Arc::clone(&self.enum_defs),
            imports: Arc::clone(&self.imports),
            types: Arc::clone(&self.types),
            type_args: self.type_args.clone(),
        }
    }

    /// The constants the body of `decl`, a `const mod`, evaluates against:
    /// its own module's.
    pub(super) fn module_of(&self, decl: &ChipDecl) -> Arc<ConstEnv> {
        if let Some(own) = self.module_by_file.get(&decl.range.file) {
            return Arc::clone(own);
        }
        match &self.mod_dups {
            Some(dups) => dups.of(&decl.range),
            None => Arc::clone(&self.module_consts),
        }
    }
}

/// The enum name an expression path's head writes, unless a constant binding
/// of the name shadows it: `Dir` in `Dir.N`, and for the head `c.Dir` of
/// `c.Dir.N`, where `c` is an `import * as` namespace whose module declares
/// enum `Dir`, that enum's own name.
fn enum_head<'e>(cx: &ConstCtx<'_>, head: &'e Expr) -> Option<&'e str> {
    match head {
        Expr::Ident { name, .. } => (!cx.consts.binds(name.as_str())).then_some(name.as_str()),
        Expr::FieldAccess { obj, field, .. } => {
            let Expr::Ident { name: ns, range } = obj.as_ref() else {
                return None;
            };
            (!cx.consts.binds(ns.as_str())
                && cx.enum_defs.namespaced(&range.file, ns, field).is_some())
            .then_some(field.as_str())
        }
        _ => None,
    }
}

/// Evaluate `e` to a constant, or explain why it is not one.
///
/// `budget` bounds every `const mod` call reached anywhere inside `e`, nested
/// arguments and interpolation slots included; it is shared by every recursive
/// call. A top-level caller passes a fresh [`Budget::default`].
pub(crate) fn eval_expr(
    e: &Expr,
    cx: &ConstCtx<'_>,
    budget: &mut Budget,
) -> Result<Literal, ConstError> {
    if let Some(lit) = expr_to_literal_in(e, &cx.consts) {
        if !matches!(e, Expr::Ident { .. }) && !super::folded_ops_certified(e, &cx.consts) {
            budget.uncertified = true;
        }
        return Ok(lit);
    }
    // Forms `expr_to_literal_in` never folds: interpolation and certified
    // method calls (through the certified evaluator), array, map and record
    // literals, and reads from them. Anything unmatched goes to `reason_for`.
    match e {
        // A bare unit variant (`None` for `Option.None`), found by the same
        // `resolve_bare_variant_enum` search typecheck and lowering use.
        Expr::Ident { name, .. } => {
            if let Some(def) = resolve_bare_variant_enum(cx, name)
                && let Some(vdef) = def.variants.iter().find(|v| &v.name == name)
            {
                return Ok(Literal::Record(vec![(
                    DISC_FIELD.to_string(),
                    Literal::Int(vdef.discriminant),
                )]));
            }
        }
        Expr::InterpLit { parts, range } => return eval_interp(parts, range, cx, budget),
        // `[a, b, ...c]`: every element must be constant. An array spread is
        // only an exec-context assignment source, so it has no constant form.
        Expr::Array { elements, .. } => {
            let mut items = Vec::with_capacity(elements.len());
            for el in elements {
                match el {
                    ArrayElem::Item(item) => items.push(eval_expr(item, cx, budget)?),
                    ArrayElem::Spread(_) => {
                        return Err(ConstError::unsupported("a spread", el.range()));
                    }
                }
            }
            return Ok(Literal::Array(items));
        }
        // `{ k => v, ... }`: every key and value must be constant.
        Expr::MapLit { entries, .. } => {
            let mut pairs = Vec::with_capacity(entries.len());
            for entry in entries {
                let key = eval_expr(&entry.key, cx, budget)?;
                let value = eval_expr(&entry.value, cx, budget)?;
                pairs.push((key, value));
            }
            return Ok(Literal::Map(pairs));
        }
        // `{ k: v, ...rest }`: fields and spreads merge in source order, as
        // typecheck infers the record type; a later entry overwrites an
        // earlier one of the same name. Spreading a non-record is refused.
        Expr::RecordLit { fields, .. } => {
            let mut entries: Vec<(String, Literal)> = Vec::new();
            let upsert =
                |entries: &mut Vec<(String, Literal)>, name: String, value: Literal| match entries
                    .iter_mut()
                    .find(|(n, _)| *n == name)
                {
                    Some(existing) => existing.1 = value,
                    None => entries.push((name, value)),
                };
            for f in fields {
                match f {
                    RecordLitField::Named { name, value, .. } => {
                        let v = eval_expr(value, cx, budget)?;
                        upsert(&mut entries, name.clone(), v);
                    }
                    // `{ x }` is `{ x: x }`.
                    RecordLitField::Shorthand { name, range } => {
                        let ident = Expr::Ident {
                            name: name.clone(),
                            range: range.clone(),
                        };
                        let v = eval_expr(&ident, cx, budget)?;
                        upsert(&mut entries, name.clone(), v);
                    }
                    RecordLitField::Spread { value, .. } => match eval_expr(value, cx, budget)? {
                        Literal::Record(spread) => {
                            for (k, v) in spread {
                                upsert(&mut entries, k, v);
                            }
                        }
                        _ => {
                            return Err(ConstError::unsupported(
                                "spreading a non-record constant",
                                value.range(),
                            ));
                        }
                    },
                }
            }
            return Ok(Literal::Record(entries));
        }
        // `rec.field` on a constant record. `obj` goes through `eval_expr`, so
        // `f().field` on a `const mod` call resolves. A missing field is
        // `RecordFieldNotFound`; a non-record `obj` is unsupported.
        Expr::FieldAccess { obj, field, range } => {
            // A namespaced constant (`Other.value`, `Other` an `import * as`
            // alias) is stored under `"Other.value"`. Resolved before `obj`,
            // which is no value; a binding named `Other` here wins instead.
            if let Expr::Ident { name: ns, .. } = obj.as_ref()
                && !cx.consts.binds(ns.as_str())
                && let Some(v) = cx.consts.get(&format!("{ns}.{field}"))
            {
                return Ok(v.clone());
            }
            // `<value>.Discriminant` and `Enum.Variant.Discriminant`, as in
            // `lower::access::lower_discriminant`. Any other `obj` (a record
            // field named `Discriminant`) falls through.
            if field == DISCRIMINANT_FIELD
                && let Some(result) = eval_discriminant(obj, cx, budget)
            {
                return result;
            }
            // A bare `Enum.Variant` (`Shape.Empty`, or `Shape.Circle` named
            // without its payload) folds to `{ __disc: N }` with no payload
            // slots, as `lower::predeclare::static_enum_ctor` does. A binding
            // shadowing the enum's name wins.
            if let Some(enum_name) = enum_head(cx, obj)
                && let Some(def) = cx.enum_defs.expression_path(enum_name)
                && let Some(vdef) = def.variants.iter().find(|v| &v.name == field)
            {
                return Ok(Literal::Record(vec![(
                    DISC_FIELD.to_string(),
                    Literal::Int(vdef.discriminant),
                )]));
            }
            return match eval_expr(obj, cx, budget)? {
                Literal::Record(entries) => match entries.into_iter().find(|(n, _)| n == field) {
                    Some((_, v)) => Ok(v),
                    None => Err(ConstError::new(
                        ConstReason::RecordFieldNotFound(field.clone()),
                        range,
                    )),
                },
                _ => Err(ConstError::unsupported(
                    "field access on this constant",
                    obj.range(),
                )),
            };
        }
        // `t[i]` and `m[k]` over a const array or map. An out-of-range runtime
        // read keeps the gate's previous value, which a constant has none of,
        // so an out-of-range index or missing key is refused (WS046).
        Expr::IndexAccess { obj, index, range } => {
            let base = eval_expr(obj, cx, budget)?;
            let idx = eval_expr(index, cx, budget)?;
            return match base {
                Literal::Array(items) => match idx {
                    Literal::Int(i) if i >= 0 && (i as usize) < items.len() => {
                        Ok(items[i as usize].clone())
                    }
                    Literal::Int(i) => Err(ConstError::new(
                        ConstReason::ArrayIndexOutOfRange {
                            index: i,
                            len: items.len(),
                        },
                        range,
                    )),
                    _ => Err(ConstError::unsupported(
                        "a non-integer array index",
                        index.range(),
                    )),
                },
                Literal::Map(pairs) => match pairs.into_iter().find(|(k, _)| *k == idx) {
                    Some((_, v)) => Ok(v),
                    None => Err(ConstError::new(ConstReason::MapKeyNotFound, range)),
                },
                // Indexing counts Unicode code points, matching the game's own
                // `String_Length`. Only a non-negative in-range index folds: the
                // Substring gate reads a negative start from the end and has its
                // own out-of-range behavior, so anything else is left to it
                // rather than guessed at compile time.
                Literal::String(s) => match idx {
                    Literal::Int(i) if i >= 0 => match s.chars().nth(i as usize) {
                        Some(c) => Ok(Literal::String(c.to_string())),
                        None => Err(ConstError::unsupported(
                            "a string index past the end",
                            range,
                        )),
                    },
                    Literal::Int(_) => Err(ConstError::unsupported(
                        "a negative string index",
                        index.range(),
                    )),
                    _ => Err(ConstError::unsupported(
                        "a non-integer string index",
                        index.range(),
                    )),
                },
                _ => Err(ConstError::unsupported(
                    "indexing this constant",
                    obj.range(),
                )),
            };
        }
        // `if <cond> then a else b`: only the taken arm is evaluated, so the
        // other may name runtime values.
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            return match eval_expr(cond, cx, budget)? {
                Literal::Bool(true) => eval_expr(then_branch, cx, budget),
                Literal::Bool(false) => eval_expr(else_branch, cx, budget),
                _ => Err(ConstError::unsupported(
                    "a non-boolean `if` condition",
                    cond.range(),
                )),
            };
        }
        // `match`: only the taken arm's body is evaluated. Uses the same
        // `lower::matchtree` decision tree as lowering's `try_const_decision`,
        // so both pick the same arm for a constant scrutinee.
        //
        // With no type inference here, an enum scrutinee's enum comes from the
        // scrutinee expression or the arms' variant citations; when neither
        // names exactly one enum, this falls through to `reason_for`.
        Expr::MatchExpr {
            scrutinee,
            arms,
            range,
        } => {
            let scrut_lit = eval_expr(scrutinee, cx, budget)?;
            // A scalar is typed by its literal. An enum prefers the one the
            // scrutinee names (`Dir.E`), so an all-unit match folds, then the
            // arms' variant citations.
            let scrut_ty = match &scrut_lit {
                Literal::Int(_) => Some(Type::Int),
                Literal::Float(_) => Some(Type::Float),
                Literal::Bool(_) => Some(Type::Bool),
                Literal::String(_) => Some(Type::String),
                Literal::Record(_) => scrutinee_enum_expr(cx, scrutinee)
                    .or_else(|| cx.enum_defs.citing_arms(arms))
                    .map(|edef| Type::Enum {
                        name: edef.name.clone(),
                        args: vec![],
                    }),
                _ => None,
            };
            if let Some(scrut_ty) = scrut_ty {
                let arm_patterns: Vec<crate::ast::Pattern> =
                    arms.iter().map(|a| a.pattern.clone()).collect();
                let guarded: Vec<bool> = arms.iter().map(|a| a.guard.is_some()).collect();
                let enums = crate::typecheck::patterns::Enums {
                    defs: &cx.enum_defs,
                    types: &cx.types,
                };
                let decision = matchtree::build_guarded(enums, &scrut_ty, &arm_patterns, &guarded);
                let mut at = &decision;
                loop {
                    match matchtree::resolve_const(at, &scrut_lit) {
                        ConstOutcome::Arm(i) => {
                            return eval_match_arm_body(&arms[i], &scrut_lit, cx, budget);
                        }
                        ConstOutcome::Fail => {
                            return Err(ConstError::unsupported(
                                "a non-exhaustive const match",
                                range,
                            ));
                        }
                        // A guard sees the arm's captures, as its body does.
                        ConstOutcome::Guard { arm, otherwise } => {
                            let Some(guard) = &arms[arm].guard else { break };
                            let inner_cx =
                                cx.with_consts(arm_capture_consts(&arms[arm], &scrut_lit, cx));
                            match eval_expr(guard, &inner_cx, budget)? {
                                Literal::Bool(true) => {
                                    return eval_match_arm_body(&arms[arm], &scrut_lit, cx, budget);
                                }
                                Literal::Bool(false) => at = otherwise,
                                _ => break,
                            }
                        }
                        ConstOutcome::Unknown => break,
                    }
                }
            }
        }
        // `value is Enum.Variant` folds as the discriminant comparison it
        // stands for.
        Expr::Is { value, path, range } => {
            let compare = Expr::variant_test_desugared(value, path, range);
            return eval_expr(&compare, cx, budget);
        }
        Expr::As { value, ty, range } => {
            let lit = eval_expr(value, cx, budget)?;
            let folded = match ty {
                crate::ast::TypeExpr::Name { name, .. } => crate::types::resolve::primitive(name)
                    .and_then(|t| crate::typecheck::convert::fold_as(&lit, &t)),
                _ => None,
            };
            return folded.ok_or_else(|| {
                ConstError::unsupported("this `as` conversion at compile time", range)
            });
        }
        // `a + b` with an operand `expr_to_literal_in` cannot fold (a `const
        // mod` call): evaluate the operands here, then apply the same
        // `eval_const_binop` law it uses.
        Expr::BinOp {
            op,
            left,
            right,
            range,
        } => {
            let l = eval_expr(left, cx, budget)?;
            let r = eval_expr(right, cx, budget)?;
            if matches!((&l, &r), (Literal::Int(_), Literal::Int(_)))
                && {
                    use crate::catalog::operators::op as o;
                    [o::ADD, o::SUB, o::MUL, o::DIV, o::REM].contains(&op.as_str())
                }
                && crate::const_eval::certified_binop(op, &l, &r).is_none()
            {
                budget.uncertified = true;
            }
            return eval_const_binop(op, l, r)
                .ok_or_else(|| ConstError::refused(binop_refused(op), range));
        }
        // `-a`, `!a` and `~a`, as `Expr::BinOp` above, via `eval_const_unop`
        // (whose `NEG` law matches `expr_to_literal_in`'s).
        Expr::UnOp { op, operand, range } => {
            let v = eval_expr(operand, cx, budget)?;
            if op == crate::catalog::operators::op::NEG && v == Literal::Int(i64::MIN) {
                budget.uncertified = true;
            }
            return eval_const_unop(op, v)
                .ok_or_else(|| ConstError::refused(unop_refused(op), range));
        }
        Expr::Call {
            callee,
            args,
            type_args,
            range,
        } => {
            if let Expr::FieldAccess { obj, field, .. } = callee.as_ref() {
                // `.length()` on a const array or map: collection methods are
                // not `catalog::calls` entries, so `eval_method_call` misses it.
                if let Some(result) = eval_collection_length(obj, field, args, cx, budget) {
                    return result;
                }
                if let Some(result) = eval_method_call(obj, field, args, range, cx, budget) {
                    return result;
                }
                // `Shape.Circle(5.0)`: positional variant construction.
                if let Some(result) = eval_enum_call_ctor(obj, field, args, range, cx, budget) {
                    return result;
                }
                // `Shape.FromInt(2)`: tag-only construction.
                if let Some(result) = eval_enum_from_int(obj, field, args, range, cx, budget) {
                    return result;
                }
                // `<enum value>.ToInt()` is `.Discriminant`.
                if field == "ToInt"
                    && args.is_empty()
                    && let Some(result) = eval_discriminant(obj, cx, budget)
                {
                    return result;
                }
            }
            // `EnumToInt(v)` and `IntToEnum(n)` fold like `v.ToInt()` and
            // `Enum.FromInt(n)`, unless a constant of that name shadows them.
            if let Expr::Ident { name, .. } = callee.as_ref()
                && !cx.consts.binds(name.as_str())
            {
                // A non-enum argument is a typecheck error, not modelled here.
                if name == "EnumToInt"
                    && let Some(CallArg::Positional(v)) = args.first()
                {
                    return eval_discriminant(v, cx, budget).unwrap_or_else(|| {
                        Err(ConstError::unsupported("EnumToInt argument", range))
                    });
                }
                // `IntToEnum(n)` folds to a tag-only `{ __disc: n }`. With a
                // `wrap` argument the gate clamps an out-of-range tag, which the
                // fold does not reproduce, so only the one-argument form folds.
                if name == "IntToEnum"
                    && args.len() == 1
                    && let Some(CallArg::Positional(v)) = args.first()
                {
                    let n = eval_expr(v, cx, budget)?;
                    return Ok(Literal::Record(vec![(DISC_FIELD.to_string(), n)]));
                }
            }
            // `Some(42)`: unqualified positional variant construction.
            if let Expr::Ident { name, .. } = callee.as_ref()
                && let Some(result) = eval_bare_variant_ctor(name, args, range, cx, budget)
            {
                return result;
            }
            // `Vec`, `Rotation` and `Color` with an argument
            // `expr_to_literal_in` cannot fold (`Vec(f(1.0), 2.0, 3.0)`):
            // evaluate the arguments here in parameter order, then apply the
            // same `fold_constructor`.
            if let Expr::Ident { name, .. } = callee.as_ref()
                && FOLDABLE_CONSTRUCTORS.contains(&name.as_str())
            {
                let lits = bind_constructor_args(name, args, range, cx, budget)?;
                return match fold_constructor(name, &lits) {
                    Some(lit) => Ok(lit),
                    None => Err(ConstError::refused(constructor_refused(name), range)),
                };
            }
            // A call to a `const mod` the calling file can name: evaluate the
            // arguments in the caller's `cx` and run the body. Any other
            // named call falls through to `reason_for` (`NotAConstMod`).
            if let Expr::Ident {
                name,
                range: name_range,
            } = callee.as_ref()
                && let Some(lookup) = cx.lookup_mod
                && let Some(decl) = lookup(name)
                && decl.is_const
                && cx.imports.user_mod_answers(&decl.range, &name_range.file)
            {
                let arg_lits = super::interp::bind_call_args(&decl.inputs, args, cx, budget)?;
                let written = super::interp::written_type_args(type_args, cx);
                return super::interp::eval_call(&decl, &arg_lits, &written, cx, budget);
            }
        }
        // `Shape.Box { w: 1.0, h: 2.0 }`: named-payload variant construction.
        // Folds only when the variant and every field resolve; otherwise it
        // falls through to `reason_for`.
        Expr::VariantCtor { path, fields, .. } => {
            if let Expr::FieldAccess {
                obj,
                field: variant,
                ..
            } = path.as_ref()
                && let Some(enum_name) = enum_head(cx, obj)
                && let Some(def) = cx.enum_defs.expression_path(enum_name)
                && let Some(vdef) = def.variants.iter().find(|v| &v.name == variant)
            {
                let mut entries = vec![(DISC_FIELD.to_string(), Literal::Int(vdef.discriminant))];
                let mut folded = true;
                for f in fields {
                    let (name, value_expr) = match f {
                        RecordLitField::Named { name, value, .. } => (name.clone(), value.clone()),
                        RecordLitField::Shorthand { name, range } => (
                            name.clone(),
                            Expr::Ident {
                                name: name.clone(),
                                range: range.clone(),
                            },
                        ),
                        RecordLitField::Spread { .. } => {
                            folded = false;
                            break;
                        }
                    };
                    match eval_expr(&value_expr, cx, budget) {
                        Ok(v) => entries.push((slot_name(variant, name), v)),
                        Err(_) => {
                            folded = false;
                            break;
                        }
                    }
                }
                if folded {
                    return Ok(Literal::Record(entries));
                }
            }
        }
        _ => {}
    }
    let (reason, range) = reason_for(e, cx);
    Err(ConstError { reason, range })
}

/// The enum a match scrutinee names directly: `Dir.E` or a bare unit variant
/// `E`. An all-unit match's arms parse as bindings, which
/// `EnumRegistry::citing_arms` cannot read, so this is what lets it fold.
/// `None` for any other shape, or when a constant shadows the enum's name.
fn scrutinee_enum_expr<'c>(cx: &'c ConstCtx<'_>, scrutinee: &Expr) -> Option<&'c EnumDef> {
    match scrutinee {
        Expr::FieldAccess { obj, .. } => {
            let Expr::Ident { name, .. } = obj.as_ref() else {
                return None;
            };
            if cx.consts.binds(name.as_str()) {
                return None;
            }
            cx.enum_defs.expression_path(name)
        }
        Expr::Ident { name, .. } => resolve_bare_variant_enum(cx, name),
        _ => None,
    }
}

/// Evaluate the taken arm's body with its payload captures bound as constants
/// on top of the caller's (see [`arm_capture_consts`]). A block-bodied arm
/// has no value and is refused.
fn eval_match_arm_body(
    arm: &MatchArm,
    scrut_lit: &Literal,
    cx: &ConstCtx<'_>,
    budget: &mut Budget,
) -> Result<Literal, ConstError> {
    let MatchBody::Expr(body) = &arm.body else {
        return Err(ConstError::unsupported(
            "a block-bodied match arm",
            &arm.range,
        ));
    };
    let inner_cx = cx.with_consts(arm_capture_consts(arm, scrut_lit, cx));
    eval_expr(body, &inner_cx, budget)
}

/// The caller's constants plus `arm`'s captures, read off the constant
/// scrutinee `scrut_lit` by `matchtree::capture_literal`.
fn arm_capture_consts(arm: &MatchArm, scrut_lit: &Literal, cx: &ConstCtx<'_>) -> ConstNames {
    let mut captures = Vec::new();
    matchtree::collect_captures(&arm.pattern, &mut Vec::new(), &mut captures);
    let mut consts = cx.consts.clone();
    for capture in captures {
        if let Some(lit) = matchtree::capture_literal(scrut_lit, &capture) {
            consts.insert(&capture.name, lit);
        }
    }
    consts
}

/// Bind a foldable constructor's arguments to its catalog parameters by name
/// and evaluate each, giving the parameter-ordered list [`fold_constructor`]
/// expects (`Vec(z = 3.0, x = 1.0, y = 2.0)` puts `1.0` on `x`).
///
/// Stricter than runtime lowering, which ignores a misfit argument: an extra
/// positional, an unknown name or a second argument for one parameter is
/// `Refused`, so a constant is never folded from whichever arguments bound.
///
/// An unbound parameter ends the list, so a hole yields a short list that
/// `fold_constructor` declines. That is sound only while every optional
/// parameter is trailing (`Color`'s `a`); the test
/// `foldable_constructors_are_all_really_foldable` checks this. Arguments past
/// a hole are not evaluated, so `Vec(x = 1.0, z = live)` is blamed on the
/// whole call (WS047) rather than on `live`.
fn bind_constructor_args(
    name: &str,
    args: &[CallArg],
    range: &SourceRange,
    cx: &ConstCtx<'_>,
    budget: &mut Budget,
) -> Result<Vec<Literal>, ConstError> {
    let refuse = || ConstError::refused(constructor_refused(name), range);
    // Present for every `FOLDABLE_CONSTRUCTORS` entry; a catalog rename
    // refuses here instead of panicking.
    let Some(spec) = crate::catalog::calls::find_call(name) else {
        return Err(refuse());
    };

    let names: Vec<&str> = spec.params.iter().map(|p| p.name).collect();
    let slots = match super::interp::bind_args(&names, 0, args, super::interp::Arity::Strict) {
        Ok(slots) => slots,
        Err(super::interp::Misfit::Argument) => return Err(refuse()),
        Err(super::interp::Misfit::Spread(v)) => {
            return Err(ConstError::unsupported("a spread", v.range()));
        }
    };

    let mut lits = Vec::with_capacity(spec.params.len());
    for slot in slots {
        // `break`, not `continue`: closing a hole shifts later arguments onto
        // the wrong slot (`Color(0.5, 0.75, a = 0.25)` would put alpha on blue).
        let Some(arg) = slot else { break };
        lits.push(eval_expr(arg, cx, budget)?);
    }
    Ok(lits)
}

/// The discriminant of `obj` for `obj.Discriminant`: `obj` is a variant path
/// (`Shape.Circle`) or an enum value. `None` for anything else (a record field
/// named `Discriminant`), so the caller treats it as an ordinary field.
fn eval_discriminant(
    obj: &Expr,
    cx: &ConstCtx<'_>,
    budget: &mut Budget,
) -> Option<Result<Literal, ConstError>> {
    // A variant path names a type, not a value, so it is read from the
    // registry, never evaluated. A binding shadowing the enum's name wins.
    if let Expr::FieldAccess {
        obj: enum_obj,
        field: variant,
        ..
    } = obj
        && let Some(enum_name) = enum_head(cx, enum_obj)
        && let Some(def) = cx.enum_defs.expression_path(enum_name)
        && let Some(vdef) = def.variants.iter().find(|v| &v.name == variant)
    {
        return Some(Ok(Literal::Int(vdef.discriminant)));
    }
    // An enum value: read its `__disc` slot, the field runtime storage
    // (`lower::predeclare::build_enum_fields`) also uses for the tag.
    match eval_expr(obj, cx, budget) {
        Ok(Literal::Record(entries)) => entries
            .into_iter()
            .find(|(n, _)| n == DISC_FIELD)
            .map(|(_, v)| Ok(v)),
        Ok(_) => None,
        Err(err) => Some(Err(err)),
    }
}

/// `Shape.Circle(5.0)`: positional variant construction. `None` when `obj` is
/// not an unshadowed enum or `variant` not one of its variants. Once the
/// variant is recognised, a non-positional or non-constant argument makes the
/// whole construction `Err` (not constant), never a partial record.
fn eval_enum_call_ctor(
    obj: &Expr,
    variant: &str,
    args: &[CallArg],
    range: &SourceRange,
    cx: &ConstCtx<'_>,
    budget: &mut Budget,
) -> Option<Result<Literal, ConstError>> {
    let enum_name = enum_head(cx, obj)?;
    let def = cx.enum_defs.expression_path(enum_name)?;
    let vdef = def.variants.iter().find(|v| v.name == variant)?;

    let mut fields = vec![(DISC_FIELD.to_string(), Literal::Int(vdef.discriminant))];
    for (i, a) in args.iter().enumerate() {
        let CallArg::Positional(v) = a else {
            // A named or spread argument on a positional variant is already a
            // typecheck error, with nothing sound to fold.
            return Some(Err(ConstError::unsupported(
                "this variant construction",
                range,
            )));
        };
        match eval_expr(v, cx, budget) {
            Ok(lit) => fields.push((slot_name(variant, i), lit)),
            Err(err) => return Some(Err(err)),
        }
    }
    Some(Ok(Literal::Record(fields)))
}

/// `Enum.FromInt(2)`: tag-only construction, folding to `{ __disc: n }` with no
/// payload slots (payloads default to zero at runtime, as in
/// `lower::expr::try_lower_enum_from_int`). `None` when `obj` is not an
/// unshadowed enum or the enum has a variant named `FromInt`. A missing or
/// non-constant argument is `Err`.
fn eval_enum_from_int(
    obj: &Expr,
    field: &str,
    args: &[CallArg],
    range: &SourceRange,
    cx: &ConstCtx<'_>,
    budget: &mut Budget,
) -> Option<Result<Literal, ConstError>> {
    if field != "FromInt" {
        return None;
    }
    let enum_name = enum_head(cx, obj)?;
    let def = cx.enum_defs.expression_path(enum_name)?;
    if def.variants.iter().any(|v| v.name == "FromInt") {
        return None; // a real variant named FromInt is an ordinary construction
    }
    // Typecheck enforces exactly one positional int argument: the tag.
    let Some(CallArg::Positional(v)) = args.first() else {
        return Some(Err(ConstError::unsupported(
            "this enum construction",
            range,
        )));
    };
    match eval_expr(v, cx, budget) {
        Ok(lit) => Some(Ok(Literal::Record(vec![(DISC_FIELD.to_string(), lit)]))),
        Err(err) => Some(Err(err)),
    }
}

/// The enum a bare variant name belongs to, through the same
/// `enums::resolve_bare_variant_enum` typecheck and lowering call, with the
/// names that shadow a variant here:
/// - any value binding of the name ([`crate::const_scope::ConstNames::binds`]);
/// - a user enum of that name (`enum Some { .. }`);
/// - a user `mod` or `chip` of that name, const or not (`lookup_mod` returns
///   every mod).
///
/// A type alias named after a variant is missed, but using one in value
/// position is already a typecheck error.
fn resolve_bare_variant_enum<'c>(
    cx: &'c ConstCtx<'_>,
    name: &str,
) -> Option<&'c crate::typecheck::enums::EnumDef> {
    crate::typecheck::enums::resolve_bare_variant_enum(&cx.enum_defs, name, |n| {
        cx.consts.binds(n)
            || cx.enum_defs.expression_path(n).is_some()
            || cx.lookup_mod.is_some_and(|f| f(n).is_some())
    })
}

/// `Some(42)`: unqualified positional variant construction, folding exactly as
/// `Option.Some(42)` does in [`eval_enum_call_ctor`]. `None` when `name` is not
/// an unshadowed, unique variant name.
fn eval_bare_variant_ctor(
    name: &str,
    args: &[CallArg],
    range: &SourceRange,
    cx: &ConstCtx<'_>,
    budget: &mut Budget,
) -> Option<Result<Literal, ConstError>> {
    let def = resolve_bare_variant_enum(cx, name)?;
    let vdef = def.variants.iter().find(|v| v.name == name)?;

    let mut fields = vec![(DISC_FIELD.to_string(), Literal::Int(vdef.discriminant))];
    for (i, a) in args.iter().enumerate() {
        let CallArg::Positional(v) = a else {
            // A named or spread argument on a positional variant is already a
            // typecheck error, with nothing sound to fold.
            return Some(Err(ConstError::unsupported(
                "this variant construction",
                range,
            )));
        };
        match eval_expr(v, cx, budget) {
            Ok(lit) => fields.push((slot_name(name, i), lit)),
            Err(err) => return Some(Err(err)),
        }
    }
    Some(Ok(Literal::Record(fields)))
}

/// `t.length()` on a const array or map: its element count. `None` when the
/// call is not `length()` or `obj` is neither, so the caller tries other
/// forms; a non-constant `obj` propagates its error.
fn eval_collection_length(
    obj: &Expr,
    field: &str,
    args: &[CallArg],
    cx: &ConstCtx<'_>,
    budget: &mut Budget,
) -> Option<Result<Literal, ConstError>> {
    if field != "length" || !args.is_empty() {
        return None;
    }
    match eval_expr(obj, cx, budget) {
        Ok(Literal::Array(items)) => Some(Ok(Literal::Int(items.len() as i64))),
        Ok(Literal::Map(pairs)) => Some(Ok(Literal::Int(pairs.len() as i64))),
        Ok(_) => None,
        Err(err) => Some(Err(err)),
    }
}

/// A method call such as `"hi".ToUpper()` or `a.Dot(b)`, folded by the
/// certified gate law. `None` for an unknown method, one with no receiver
/// form, or an exec call, so `reason_for` explains it. Otherwise `Err` is a
/// non-constant operand's own error, or `Refused` (WS047) naming the method.
///
/// Binds as `lower::call::builtin::lower_builtin_call` does (the receiver is
/// the first parameter), so the inputs match the gate's ports; an unbound
/// optional parameter is an unwired (`None`) input.
fn eval_method_call(
    obj: &Expr,
    field: &str,
    args: &[CallArg],
    range: &SourceRange,
    cx: &ConstCtx<'_>,
    budget: &mut Budget,
) -> Option<Result<Literal, ConstError>> {
    let spec = crate::catalog::calls::find_call(field)?;
    if spec.exec || spec.receiver.is_none() {
        return None;
    }
    if spec.params.is_empty() {
        return None;
    }

    let names: Vec<&str> = spec.params.iter().map(|p| p.name).collect();
    let Ok(mut slots) = super::interp::bind_args(&names, 1, args, super::interp::Arity::Lenient)
    else {
        unreachable!("lenient binding refuses nothing")
    };
    slots[0] = slots[0].or(Some(obj));

    let refuse = |why: String| Some(Err(ConstError::refused(why, range)));

    let mut inputs: Vec<Option<Value>> = Vec::with_capacity(spec.params.len());
    for slot in slots {
        let Some(arg_expr) = slot else {
            inputs.push(None); // unwired, exactly like an omitted optional arg
            continue;
        };
        let lit = match eval_expr(arg_expr, cx, budget) {
            Ok(lit) => lit,
            Err(err) => return Some(Err(err)),
        };
        let Some(value) = Value::from_literal(&lit) else {
            return refuse(format!(
                "`.{field}(…)` has no certified constant result for these operands"
            ));
        };
        inputs.push(Some(value));
    }

    // `fold_eval::eval` computes one gate's certified value law from
    // evaluated operands; it does not execute source text.
    match fold_eval::eval(spec.gate_class, &inputs) {
        Some(v) => Some(Ok(v.to_literal())),
        None => refuse(format!(
            "`.{field}(…)` has no certified constant result for these operands"
        )),
    }
}

/// `"a${1 + 1}b"`: renders the same `{N}` templates `lower::ops::lower_interp`
/// builds through `fold::eval::format_text`, so the constant matches what the
/// `FormatText` gates print. Groups hold 7 slots (`InputA` to `InputG`); their
/// texts join by plain concatenation, which is `Concatenate`'s certified law
/// for two strings.
fn eval_interp(
    parts: &[InterpPart],
    range: &SourceRange,
    cx: &ConstCtx<'_>,
    budget: &mut Budget,
) -> Result<Literal, ConstError> {
    let groups = fold_eval::interp_groups(parts, |expr| {
        // A failing part's error already blames the right sub-range.
        let lit = eval_expr(expr, cx, budget)?;
        Value::from_literal(&lit).ok_or_else(|| {
            ConstError::refused(
                "this interpolated value has no certified constant text form",
                expr.range(),
            )
        })
    })?;

    let mut out = String::new();
    for (fmt, slots) in groups {
        let inputs: Vec<Option<Value>> = slots.into_iter().map(Some).collect();
        match fold_eval::format_text(&fmt, &inputs) {
            Some(s) => out.push_str(&s),
            None => {
                return Err(ConstError::refused(
                    "this string interpolation has no certified constant result",
                    range,
                ));
            }
        }
    }
    Ok(Literal::String(out))
}

/// Why `e` is not constant, with the range of the sub-expression to blame: the
/// first part that fails to evaluate (`a` in `a + 1`, `runtimeThing` in
/// `Vec(runtimeThing, 1.0, 2.0)`). When every part is constant, the operator
/// or constructor declined them (`1 << 100`): `Refused`, on the whole `e`.
fn reason_for(e: &Expr, cx: &ConstCtx<'_>) -> (ConstReason, SourceRange) {
    let fails = |e: &Expr| expr_to_literal_in(e, &cx.consts).is_none();
    match e {
        Expr::Ident { name, .. } if !cx.consts.contains_key(name.as_str()) => {
            (ConstReason::NotConstant(name.clone()), e.range().clone())
        }
        Expr::BinOp {
            op, left, right, ..
        } => {
            if fails(left) {
                reason_for(left, cx)
            } else if fails(right) {
                reason_for(right, cx)
            } else {
                (ConstReason::Refused(binop_refused(op)), e.range().clone())
            }
        }
        Expr::UnOp { op, operand, .. } => {
            if fails(operand) {
                reason_for(operand, cx)
            } else {
                (ConstReason::Refused(unop_refused(op)), e.range().clone())
            }
        }
        Expr::Call { callee, args, .. } => {
            // An argument is the likelier culprit than the callee, and naming
            // the callee first would misreport `Vec(runtimeThing, 1.0, 2.0)`.
            for a in args {
                let value = match a {
                    CallArg::Positional(v) | CallArg::Spread(v) => v,
                    CallArg::Named { value, .. } => value,
                };
                if fails(value) {
                    return reason_for(value, cx);
                }
            }
            match callee.as_ref() {
                Expr::Ident { name, .. } if FOLDABLE_CONSTRUCTORS.contains(&name.as_str()) => (
                    ConstReason::Refused(constructor_refused(name)),
                    e.range().clone(),
                ),
                // An unevaluated call is either not a `const mod`, or a
                // `const mod` nested in something with no compile-time form,
                // so evaluation never reached it. Resolve the callee to tell.
                Expr::Ident {
                    name,
                    range: name_range,
                } => {
                    let decl = cx
                        .lookup_mod
                        .and_then(|lookup| lookup(name))
                        .filter(|decl| decl.is_const);
                    let reason = match decl {
                        Some(decl)
                            if cx.imports.user_mod_answers(&decl.range, &name_range.file) =>
                        {
                            ConstReason::NestedConstModCall(name.clone())
                        }
                        Some(_) => ConstReason::UnnamedConstMod(name.clone()),
                        None => ConstReason::NotAConstMod(name.clone()),
                    };
                    (reason, e.range().clone())
                }
                _ => (ConstReason::Unsupported("this call"), e.range().clone()),
            }
        }
        Expr::InterpLit { .. } => (
            ConstReason::Unsupported("string interpolation"),
            e.range().clone(),
        ),
        Expr::IndexAccess { .. } => (ConstReason::Unsupported("indexing"), e.range().clone()),
        Expr::IfExpr { .. } => (
            ConstReason::Unsupported("an `if` expression"),
            e.range().clone(),
        ),
        Expr::RecordLit { .. } => (
            ConstReason::Unsupported("a record literal"),
            e.range().clone(),
        ),
        Expr::FieldAccess { .. } => (ConstReason::Unsupported("field access"), e.range().clone()),
        Expr::VariantCtor { .. } => (
            ConstReason::Unsupported("a variant construction"),
            e.range().clone(),
        ),
        _ => (
            ConstReason::Unsupported("this expression"),
            e.range().clone(),
        ),
    }
}
