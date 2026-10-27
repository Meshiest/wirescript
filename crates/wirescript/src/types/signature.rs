//! The shape of a callable: its parameters in order and what each binds to.
//! Typecheck checks a call's arguments against it (`typecheck::check_args`).

use crate::ir::Type;

/// What a `Param` binds to, and how `check_args` should validate an argument
/// against it. Each config variant carries the gate PORT name as its
/// `&'static str` payload (the key for the schema/shape lookup); `Param::name`
/// stays the SURFACE name (the key for named-arg matching + diagnostics), so
/// the two never have to share one field.
#[derive(Clone, Debug)]
pub enum ParamKind {
    /// An ordinary wire-input param: infer the argument's type and `coerce`
    /// it against `Param::ty`.
    Wire,
    /// A data-only (non-wire) param backed by a schema enum field. The
    /// `&'static str` is the enum type name (`crate::catalog::enum_*`
    /// lookups).
    ConfigEnum(&'static str),
    /// A wire-input param whose gate data field is also a schema enum
    /// (`DisplayText`'s `justify`/`easing`). A constant enum form validates like
    /// [`ParamKind::ConfigEnum`] and bakes into the field; any other value is
    /// checked like [`ParamKind::Wire`]. The payload is the enum type name.
    WireEnum(&'static str),
    /// A composite constant-only config param (`meshColors: Color[]`,
    /// `ammoOverride`'s nested struct) that must fold to a constant of the
    /// expected shape. The `&'static str` is the gate PORT name the composite
    /// shape is keyed on (`"MeshColors"` / `"WeaponAmmoOverride"`).
    ConfigComposite(&'static str),
    /// A plain scalar/asset constant-only config param; must fold to a
    /// literal. The `&'static str` is the gate port name (carried for
    /// symmetry; the constant check itself needs only the value).
    ConfigScalar(&'static str),
    /// A user `const` parameter (`name: const string`, or any parameter of a
    /// `const mod`). The argument must evaluate at compile time; it is ALSO
    /// type-checked exactly like a `Wire` param, since a const value still has
    /// a type.
    Const,
}

/// One parameter of a `CallSignature`.
#[derive(Clone, Debug)]
pub struct Param {
    /// Surface parameter name, always the source-level name. Used both for
    /// named-arg matching (`meshColors = ...`) and in diagnostics
    /// (`argument '{name}': ...`). Gate port names for config schema lookups
    /// live on the `ParamKind` variant payload, never here.
    pub name: String,
    pub ty: Type,
    /// When true, callers may omit the argument.
    pub optional: bool,
    pub kind: ParamKind,
}

/// A callable shape: name, params (positional-order), and, for
/// builtins/receivers backed by a gate, the gate class that data-driven config
/// (`config` array from the inventory) validates against. The call's result
/// type is derived separately by each caller (e.g. `output_record_type`,
/// `array_return_type`, `map_return_type`), not carried on this struct.
pub struct CallSignature {
    pub name: String,
    pub params: Vec<Param>,
    /// Gate class for data-driven settings config (`<FieldName> = value`
    /// named args not covered by a declared `Param`). `None` for call forms
    /// with no backing gate (e.g. plain math).
    pub config_gate: Option<&'static str>,
}
