//! Compile-time evaluation, shared by typecheck and lowering.
//!
//! Both must agree exactly: `const if` drops a branch during lowering, and the
//! typechecker must skip the same branch, so both evaluate through this module.

use std::sync::Arc;

use crate::ast::{Expr, ImportScope, LetDecl, TopDecl, TypeAliasDecl, TypeExpr};
use crate::collections::HashMap;
use crate::diagnostic::SourceRange;
use crate::ir::{Literal, Type};
use crate::typecheck::enums::{DISC_FIELD, EnumRegistry, Payload, slot_name};
use crate::types::infer::Subst;
use crate::types::resolve::{AliasDef, GenericAlias, TypeNames};

mod destructure;
mod env;
mod error;
mod expr;
mod interp;
mod literal;

pub(crate) use destructure::{bind_destructured, bound_names};
#[cfg(test)]
pub(crate) use env::build_const_env;
pub(crate) use env::{
    ConstEnv, DupConsts, build_const_declared_names, build_const_env_at, build_const_env_in,
    build_const_uncertified, dup_key,
};
pub(crate) use error::{ConstError, ConstReason};
pub(crate) use expr::{ConstCtx, eval_expr};
pub(crate) use expr::{ModDups, ModuleConsts};
pub(crate) use interp::Budget;
pub(crate) use literal::{
    certified_binop, default_literal_for_var_type, eval_const_binop, eval_const_unop,
    expr_to_literal, expr_to_literal_in, fold_ammo_override, fold_constructor, fold_mesh_colors,
    holds_refs, ref_shaped,
};

/// Whether every integer `+`, `-`, `*`, `/`, `%` and negation `e` evaluates,
/// in called `const mod` bodies too, has a certified gate result. Overflow and
/// mixed-sign division with a remainder have none, so such a value is not
/// baked; division or remainder by zero is 0, as the gate gives. Names are
/// taken as `cx` holds them (`Admit::Certified` leaves uncertified ones out).
pub(crate) fn integer_ops_certified(e: &Expr, cx: &ConstCtx<'_>) -> bool {
    let mut budget = Budget::default();
    eval_expr(e, cx, &mut budget).is_ok() && !budget.uncertified
}

/// Whether each integer operation in `e`, which the narrow evaluator folds on
/// its own against `consts`, has the result the certified fold table gives
/// (see [`integer_ops_certified`]).
pub(crate) fn folded_ops_certified<C>(e: &Expr, consts: &C) -> bool
where
    C: crate::const_scope::Constants + ?Sized,
{
    use crate::ast::walk::{Ctx, Node, walk};
    use crate::catalog::operators::op;
    let int = |e: &Expr| match expr_to_literal_in(e, consts) {
        Some(l @ Literal::Int(_)) => Some(l),
        _ => None,
    };
    let arithmetic = |o: &str| [op::ADD, op::SUB, op::MUL, op::DIV, op::REM].contains(&o);
    let certified = |n: &Expr| match n {
        Expr::BinOp {
            op: o, left, right, ..
        } if arithmetic(o) => match (int(left), int(right)) {
            (Some(l), Some(r)) => certified_binop(o, &l, &r).is_some(),
            _ => true,
        },
        Expr::UnOp { op: o, operand, .. } => {
            !(o == op::NEG && int(operand) == Some(Literal::Int(i64::MIN)))
        }
        _ => true,
    };
    let mut ok = certified(e);
    walk(Node::Expr(e), Ctx::default(), &mut |n, _| {
        if let Node::Expr(x) = n {
            ok = ok && certified(x);
        }
        ok
    });
    ok
}

/// The value a `let`/`const` binds: its initializer's, held in the type its
/// annotation names when it has one (see [`coerce_const_to_type`]).
pub(crate) fn eval_let_value(
    l: &LetDecl,
    cx: &ConstCtx<'_>,
    budget: &mut Budget,
) -> Result<Literal, error::ConstError> {
    // An annotated `null` is the zero of its annotation's type.
    if let (Expr::NullLit { .. }, Some(te)) = (&l.value, &l.typ) {
        let types = cx.types.in_file(&l.range.file);
        let t = crate::types::mono::unwrap_ref(&types.resolve_in(te, &cx.type_args));
        if matches!(
            t,
            Type::Int
                | Type::Float
                | Type::Bool
                | Type::String
                | Type::Vector
                | Type::Rotator
                | Type::Quat
                | Type::Color
                | Type::Entity
                | Type::Character
                | Type::Controller
        ) && let Some(lit) = default_literal_for_var_type(&t)
        {
            return Ok(lit);
        }
    }
    let lit = eval_expr(&l.value, cx, budget)?;
    Ok(match &l.typ {
        Some(te) => {
            let types = cx.types.in_file(&l.range.file);
            coerce_const_to_type(lit, &types.resolve_in(te, &cx.type_args), types)
        }
        None => lit,
    })
}

/// `lit`, the value of `l`'s initializer, held in the type `l`'s annotation
/// names when it has one.
pub(crate) fn in_let_annotation(l: &LetDecl, lit: Literal, types: &DeclaredTypes) -> Literal {
    match &l.typ {
        Some(te) => {
            let types = types.in_file(&l.range.file);
            coerce_const_to_type(lit, &types.resolve(te), types)
        }
        None => lit,
    }
}

/// The integer a float constant is where its type is declared `int`: the
/// value truncated toward zero, and whether that keeps the value written
/// (`false` when a fraction is dropped, `WS106`). `None` when no `int` holds
/// it: beyond the 64-bit range, or not finite (`WS028`).
pub(crate) fn float_as_int(f: f64) -> Option<(i64, bool)> {
    // 2^63, the first float past `i64::MAX`; `-2^63` is `i64::MIN` exactly.
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    (f.is_finite() && (-LIMIT..LIMIT).contains(&f)).then(|| (f.trunc() as i64, f.fract() == 0.0))
}

/// Whether the number literal `text` (`2.5`, `1e18`, `-4503599627370497.5`)
/// is exactly the integer `n`: no nonzero digit after its point, and its
/// whole part `n`'s digits.
pub(crate) fn literal_is_integer(text: &str, n: i64) -> bool {
    let text: String = text.chars().filter(|c| *c != '_').collect();
    let (negative, text) = match text.strip_prefix('-') {
        Some(rest) => (true, rest.to_string()),
        None => (false, text),
    };
    let (mantissa, exp) = match text.split_once(['e', 'E']) {
        Some((m, x)) => match x.parse::<i64>() {
            Ok(x) => (m.to_string(), x),
            Err(_) => return true,
        },
        None => (text.clone(), 0),
    };
    let (whole, frac) = mantissa.split_once('.').unwrap_or((&mantissa, ""));
    let digits: Vec<u8> = whole.bytes().chain(frac.bytes()).collect();
    if !digits.iter().all(u8::is_ascii_digit) {
        return true;
    }
    let point = whole.len() as i64 + exp;
    let split = point.clamp(0, digits.len() as i64) as usize;
    if digits[split..].iter().any(|d| *d != b'0') {
        return false;
    }
    let mut int_digits: String = digits[..split].iter().map(|d| *d as char).collect();
    int_digits.extend(std::iter::repeat_n(
        '0',
        (point - split as i64).max(0) as usize,
    ));
    let int_digits = int_digits.trim_start_matches('0');
    let want = n.unsigned_abs().to_string();
    let sign_ok = n == 0 || negative == (n < 0);
    sign_ok && (int_digits == want || (int_digits.is_empty() && n == 0))
}

pub(crate) use crate::ir::written_float;

/// A number constant where its type is declared the number type `declared`:
/// an integer is the float it names in a `float`, and a float the integer it
/// truncates to in an `int` ([`float_as_int`]; one no `int` holds stays a
/// float, which typecheck reports). Anything else is unchanged.
pub(crate) fn number_in_type(lit: Literal, declared: &Type) -> Literal {
    match (lit, declared) {
        (Literal::Int(n), Type::Float) => Literal::Float(n as f64),
        (Literal::Float(f), Type::Int) => match float_as_int(f) {
            Some((n, _)) => Literal::Int(n),
            None => Literal::Float(f),
        },
        (lit, _) => lit,
    }
}

/// A scalar literal as a slot of scalar type `ty` holds it, by the gates'
/// conversion laws: an int and a float convert as in [`number_in_type`], a
/// bool is the number 1 or 0, a number is the bool `!= 0`, and a string is the
/// bool `!= ""`. Any other pair, and any non-scalar type, is unchanged.
///
/// Emit applies the same laws to array elements (`wire_value_as`), so a value
/// baked here and one converted there agree.
pub(crate) fn scalar_in_type(lit: Literal, ty: &Type) -> Literal {
    match (lit, ty) {
        (lit @ (Literal::Int(_) | Literal::Float(_)), Type::Int | Type::Float) => {
            number_in_type(lit, ty)
        }
        (Literal::Bool(b), Type::Float) => Literal::Float(if b { 1.0 } else { 0.0 }),
        (Literal::Bool(b), Type::Int) => Literal::Int(i64::from(b)),
        (Literal::Int(n), Type::Bool) => Literal::Bool(n != 0),
        (Literal::Float(f), Type::Bool) => Literal::Bool(f != 0.0),
        (Literal::String(s), Type::Bool) => Literal::Bool(!s.is_empty()),
        (lit, _) => lit,
    }
}

/// The value the constant `lit` takes where its type is declared `declared`:
/// each number becomes the declared number type ([`number_in_type`]), through
/// record fields, tuple, array and map elements, and enum payloads. Anything
/// else is unchanged.
///
/// Every constant with a declared type goes through this function (annotated
/// `const`/`let`, `const` parameters, a `const mod`'s parameters and result,
/// and `int_constants_in_slot`), so no other route converts a constant.
pub(crate) fn coerce_const_to_type(
    lit: Literal,
    declared: &Type,
    types: &DeclaredTypes,
) -> Literal {
    let declared = match declared {
        Type::Ref(inner) => inner.as_ref(),
        other => other,
    };
    let field = |slots: &[(String, Type)], name: &str, l: Literal| match slots
        .iter()
        .find(|(n, _)| n == name)
    {
        Some((_, t)) => coerce_const_to_type(l, t, types),
        None => l,
    };
    match (lit, declared) {
        (lit @ (Literal::Int(_) | Literal::Float(_)), number @ (Type::Int | Type::Float)) => {
            number_in_type(lit, number)
        }
        (Literal::Array(items), Type::Array(elem)) => Literal::Array(
            items
                .into_iter()
                .map(|l| coerce_const_to_type(l, elem, types))
                .collect(),
        ),
        (Literal::Map(pairs), Type::Map(k, v)) => Literal::Map(
            pairs
                .into_iter()
                .map(|(kl, vl)| {
                    (
                        coerce_const_to_type(kl, k, types),
                        coerce_const_to_type(vl, v, types),
                    )
                })
                .collect(),
        ),
        (Literal::Record(fields), Type::Record(slots)) => Literal::Record(
            fields
                .into_iter()
                .map(|(name, l)| {
                    let l = field(slots, &name, l);
                    (name, l)
                })
                .collect(),
        ),
        (Literal::Record(fields), Type::Tuple(slots)) => Literal::Record(
            fields
                .into_iter()
                .map(
                    |(name, l)| match name.parse::<usize>().ok().and_then(|i| slots.get(i)) {
                        Some(t) => {
                            let l = coerce_const_to_type(l, t, types);
                            (name, l)
                        }
                        None => (name, l),
                    },
                )
                .collect(),
        ),
        (Literal::Record(fields), t @ Type::Enum { args, .. }) => {
            let slots = types.payload_slots(t, args, &fields);
            Literal::Record(
                fields
                    .into_iter()
                    .map(|(name, l)| {
                        let l = field(&slots, &name, l);
                        (name, l)
                    })
                    .collect(),
            )
        }
        (lit, _) => lit,
    }
}

/// The type declarations a constant's declared type is read against: the
/// program's type aliases (generic ones included) and enums, by the name a
/// declaration writes them with, and those each imported file can name by
/// its file.
#[derive(Default)]
pub(crate) struct DeclaredTypes {
    aliases: HashMap<String, (Vec<String>, TypeExpr)>,
    /// Each alias's place in declaration order.
    order: HashMap<String, usize>,
    enum_defs: Arc<EnumRegistry>,
    by_file: HashMap<Arc<str>, Self>,
}

/// How deep an alias may expand inside another before it is taken as `any`.
const MAX_DECLARED_DEPTH: u32 = 32;

/// What a type parameter stands for in a [`DeclaredScope`].
#[derive(Clone, Copy)]
enum Param<'a> {
    /// Whatever the caller answers for a name at a range.
    Fn(&'a dyn Fn(&str, &SourceRange) -> Option<Type>),
    /// A generic alias's arguments, by its parameter names.
    Bound(&'a HashMap<String, Type>),
}

/// A [`DeclaredTypes`] table as the resolver reads it, with what its type
/// parameters stand for. A parameter is read before anything else a name
/// could mean, a primitive included.
struct DeclaredScope<'a> {
    table: &'a DeclaredTypes,
    param: Param<'a>,
}

impl TypeNames for DeclaredScope<'_> {
    fn param(&self, name: &str, range: &SourceRange) -> Option<Type> {
        match self.param {
            Param::Fn(f) => f(name, range),
            Param::Bound(b) => b.get(name).cloned(),
        }
    }

    fn alias(&self, name: &str) -> Option<AliasDef<'_>> {
        let (params, body) = self.table.aliases.get(name)?;
        Some(if params.is_empty() {
            AliasDef::Body(body)
        } else {
            AliasDef::Generic(params, body)
        })
    }

    fn enum_named(&self, name: &str, args: Vec<Type>) -> Option<Type> {
        self.table.enum_defs.type_named(name, args)
    }

    /// An alias body is written at the top level of its own file, so it
    /// reads its names there, with none of the type parameters of the place
    /// that uses the alias.
    fn body_scope<'s>(
        &'s self,
        bindings: &'s HashMap<String, Type>,
        _: &'s [String],
    ) -> Box<dyn TypeNames + 's> {
        Box::new(DeclaredScope {
            table: self.table,
            param: Param::Bound(bindings),
        })
    }
}

impl DeclaredTypes {
    /// The declarations of the program whose top level is `decls`, and of
    /// each file it imports (see [`crate::ast::ImportScope::aliases`]).
    pub(crate) fn for_program_in(
        decls: &[TopDecl],
        enum_defs: &Arc<EnumRegistry>,
        imports: &ImportScope,
    ) -> Self {
        let mut out = Self::for_program(decls, enum_defs);
        for (file, own) in &imports.aliases {
            out.by_file
                .insert(Arc::clone(file), Self::for_program(own, enum_defs));
        }
        out
    }

    /// The declarations of the program whose top level is `decls`.
    pub(crate) fn for_program(decls: &[TopDecl], enum_defs: &Arc<EnumRegistry>) -> Self {
        let mut out = Self {
            enum_defs: Arc::clone(enum_defs),
            ..Self::default()
        };
        let alias = |ta: &TypeAliasDecl| {
            (
                ta.type_params
                    .iter()
                    .map(|p| p.name.clone())
                    .collect::<Vec<_>>(),
                ta.typ.clone(),
            )
        };
        for d in decls {
            match d {
                TopDecl::TypeAlias(ta) => {
                    out.order.insert(ta.name.clone(), out.order.len());
                    out.aliases.insert(ta.name.clone(), alias(ta));
                }
                TopDecl::Namespace(ns) => {
                    for nd in &ns.decls {
                        if let TopDecl::TypeAlias(ta) = nd {
                            let key = format!("{}.{}", ns.name, ta.name);
                            out.order.insert(key.clone(), out.order.len());
                            out.aliases.insert(key, alias(ta));
                        }
                        // The module's enum, registered under its own name, is
                        // the type `Ns.Name`.
                        if let TopDecl::Enum(e) = nd {
                            let params: Vec<String> =
                                e.type_params.iter().map(|p| p.name.clone()).collect();
                            let name = |n: &str| TypeExpr::Name {
                                name: n.to_string(),
                                range: e.range.clone(),
                            };
                            let body = if params.is_empty() {
                                name(&e.name)
                            } else {
                                TypeExpr::Generic {
                                    name: e.name.clone(),
                                    args: params.iter().map(|p| name(p)).collect(),
                                    range: e.range.clone(),
                                }
                            };
                            out.aliases
                                .insert(format!("{}.{}", ns.name, e.name), (params, body));
                        }
                    }
                    if let Some(file) = ns.decls.first().map(|d| Arc::clone(&d.range().file)) {
                        out.by_file
                            .insert(file, Self::for_program(&ns.decls, enum_defs));
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// The declarations a type written in `file` is read against: that
    /// file's own when `file` is an imported file, the program's otherwise.
    pub(crate) fn in_file(&self, file: &str) -> &Self {
        if let Some(own) = self.by_file.get(file) {
            return own;
        }
        for module in self.by_file.values() {
            let found = module.in_file(file);
            if !std::ptr::eq(found, module) {
                return found;
            }
        }
        self
    }

    /// Where alias `name` is declared among the program's aliases: `0` for
    /// the first one.
    pub(crate) fn declared_at(&self, name: &str) -> Option<usize> {
        self.order.get(name).copied()
    }

    /// The type parameters and body of the alias `name`.
    pub(crate) fn alias(&self, name: &str) -> Option<(&[String], &TypeExpr)> {
        self.aliases
            .get(name)
            .map(|(params, body)| (params.as_slice(), body))
    }

    /// What the non-generic alias `name` finally stands for: its body, or,
    /// when that body only names another alias, that alias's, as far as the
    /// chain goes. A member of an `import * as` module reads a sibling under
    /// its own qualified name first.
    pub(crate) fn alias_target(&self, name: &str) -> Option<&TypeExpr> {
        let (params, mut body) = self.alias(name)?;
        if !params.is_empty() {
            return None;
        }
        let own = |n: &str| name.rsplit_once('.').map(|(ns, _)| format!("{ns}.{n}"));
        for _ in 0..MAX_DECLARED_DEPTH {
            let TypeExpr::Name { name: next, .. } = body else {
                break;
            };
            if next == name {
                break;
            }
            match own(next)
                .and_then(|q| self.alias(&q))
                .or_else(|| self.alias(next))
            {
                Some(([], b)) => body = b,
                _ => break,
            }
        }
        Some(body)
    }

    /// Each non-generic alias of this table with the type it names.
    pub(crate) fn plain_aliases(&self) -> impl Iterator<Item = (String, Type)> + '_ {
        self.aliases
            .iter()
            .filter(|(_, (params, _))| params.is_empty())
            .map(|(name, (_, body))| (name.clone(), self.resolve(body)))
    }

    /// Each generic alias of this table with its parameters and body.
    pub(crate) fn generic_aliases(&self) -> impl Iterator<Item = (String, GenericAlias)> + '_ {
        self.aliases
            .iter()
            .filter(|(_, (params, _))| !params.is_empty())
            .map(|(name, (params, body))| {
                (
                    name.clone(),
                    GenericAlias {
                        params: params.clone(),
                        body: body.clone(),
                    },
                )
            })
    }

    /// The type `te` names.
    pub(crate) fn resolve(&self, te: &TypeExpr) -> Type {
        self.resolve_in(te, &Subst::default())
    }

    /// The type `te` names where each type parameter in `subst` stands for its
    /// type.
    pub(crate) fn resolve_in(&self, te: &TypeExpr, subst: &Subst) -> Type {
        self.resolve_scoped(te, &|name, _| subst.get(name).cloned())
    }

    /// The type `te` names, where `param` answers what a name written at a
    /// range stands for when it is a type parameter there. Only the names `te`
    /// itself writes are asked: an alias body is written at the top level of
    /// its own file, so it reads its names there, with none of the type
    /// parameters of the place that uses the alias.
    pub(crate) fn resolve_scoped(
        &self,
        te: &TypeExpr,
        param: &dyn Fn(&str, &SourceRange) -> Option<Type>,
    ) -> Type {
        crate::types::resolve::resolve_with(
            te,
            &DeclaredScope {
                table: self,
                param: Param::Fn(param),
            },
            &mut Vec::new(),
        )
    }

    /// The declared type of each slot of the enum value `fields` (keyed
    /// `__{Variant}_{field}`) of the enum type `t` instantiated with `args`.
    fn payload_slots(
        &self,
        t: &Type,
        args: &[Type],
        fields: &[(String, Literal)],
    ) -> Vec<(String, Type)> {
        let Some(def) = self.enum_defs.of(t) else {
            return Vec::new();
        };
        let disc = fields.iter().find_map(|(n, l)| match (n.as_str(), l) {
            (n, Literal::Int(d)) if n == DISC_FIELD => Some(*d),
            _ => None,
        });
        let Some(variant) = def.variants.iter().find(|v| Some(v.discriminant) == disc) else {
            return Vec::new();
        };
        let subst: Subst = def
            .type_params
            .iter()
            .map(|p| p.name.clone())
            .zip(args.iter().cloned())
            .collect();
        let slot = |f: &dyn std::fmt::Display, te: &TypeExpr| {
            (slot_name(&variant.name, f), self.resolve_in(te, &subst))
        };
        match &variant.payload {
            Payload::Unit => Vec::new(),
            Payload::Positional(ts) => ts.iter().enumerate().map(|(i, te)| slot(&i, te)).collect(),
            Payload::Named(fs) => fs.iter().map(|(n, te)| slot(n, te)).collect(),
        }
    }
}

#[cfg(test)]
mod tests;
