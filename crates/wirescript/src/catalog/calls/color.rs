//! Color calls: sRGB/hex conversion, blending, and channel split/convert.

use super::*;

pub(super) fn register(m: &mut HashMap<&'static str, CallSpec>) {
    // ---- sRGB / hex color ----------------------------------
    m.insert(
        "ColorSRGB",
        vec_expr(
            "ColorSRGB",
            gc::MAKE_COLOR_SRGB,
            vec![
                CallParam::req("r", WirePort::R, Type::Int),
                CallParam::req("g", WirePort::G, Type::Int),
                CallParam::req("b", WirePort::B, Type::Int),
                CallParam::req("a", WirePort::A, Type::Int),
            ],
            WirePort::Output,
            Type::Color,
        ),
    );
    m.insert(
        "ColorHex",
        vec_expr(
            "ColorHex",
            gc::MAKE_COLOR_HEX,
            vec![CallParam::req("hex", WirePort::Hex, Type::String)],
            WirePort::Output,
            Type::Color,
        ),
    );
    m.insert(
        "ToSRGB",
        expr_recv(
            "ToSRGB",
            gc::SPLIT_COLOR_SRGB,
            Type::Color,
            vec![CallParam::req("c", WirePort::Input, Type::Color)],
            WirePort::R,
            Type::Record(vec![
                ("R".into(), Type::Int),
                ("G".into(), Type::Int),
                ("B".into(), Type::Int),
                ("A".into(), Type::Int),
            ]),
        ),
    );
    m.insert(
        "ToHex",
        expr_recv(
            "ToHex",
            gc::COLOR_TO_HEX,
            Type::Color,
            vec![CallParam::req("c", WirePort::Input, Type::Color)],
            WirePort::Hex,
            Type::String,
        ),
    );
    // `Blend` is the math blend gate — an alias for `lerp`, taking any of the
    // math variants. `ColorBlend` below is a DIFFERENT gate (it carries a
    // colour-space selection), so both stay reachable.
    m.insert(
        "Blend",
        expr_recv(
            "Blend",
            gc::MATH_BLEND,
            blend_variant(),
            vec![
                CallParam::req("a", WirePort::InputA, blend_variant()),
                CallParam::req("b", WirePort::InputB, blend_variant()),
                CallParam::req("alpha", WirePort::Blend, Type::Float),
                // Config-only (settings menu, not a wire input).
                CallParam::opt("clampAlpha", WirePort::BClampAlpha, Type::Bool),
            ],
            WirePort::Output,
            blend_variant(),
        ),
    );
    m.insert(
        "ColorBlend",
        expr_recv(
            "ColorBlend",
            gc::COLOR_BLEND,
            Type::Color,
            vec![
                CallParam::req("a", WirePort::ColorA, Type::Color),
                CallParam::req("b", WirePort::ColorB, Type::Color),
                CallParam::req("alpha", WirePort::Alpha, Type::Float),
                // Config-only (settings menu, not wire inputs). `blendSpace`
                // is an EBRColorSpace enum member.
                CallParam::opt("blendSpace", WirePort::BlendSpace, Type::Int),
                CallParam::opt("clampAlpha", WirePort::BClampAlpha, Type::Bool),
            ],
            WirePort::Output,
            Type::Color,
        ),
    );

    // ---- Color ----------------------------------------------------------
    m.insert(
        "Color",
        vec_expr(
            "Color",
            gc::MAKE_COLOR,
            vec![
                CallParam::req("r", WirePort::R, Type::Float),
                CallParam::req("g", WirePort::G, Type::Float),
                CallParam::req("b", WirePort::B, Type::Float),
                CallParam::opt("a", WirePort::A, Type::Float),
            ],
            WirePort::Output,
            Type::Color,
        ),
    );

    m.insert(
        "SplitColor",
        CallSpec {
            name: "SplitColor",
            gate_class: gc::SPLIT_COLOR,
            params: vec![CallParam::req("c", WirePort::Input, Type::Color)],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::R,
                ty: Type::Record(vec![
                    ("r".into(), Type::Float),
                    ("g".into(), Type::Float),
                    ("b".into(), Type::Float),
                    ("a".into(), Type::Float),
                ]),
            }],
            receiver: Some(Type::Color),
        },
    );

    // Color-space conversion: `col.ConvertColor(fromSpace, toSpace)`. The spaces
    // are constant-only enum data fields (see DATA_ONLY_PARAMS).
    m.insert(
        "ConvertColor",
        CallSpec {
            name: "ConvertColor",
            gate_class: gc::EXPR_COLOR_CONVERT,
            params: vec![
                CallParam::req("color", WirePort::Input, Type::Color),
                CallParam::opt("fromSpace", WirePort::FromSpace, Type::Int),
                CallParam::opt("toSpace", WirePort::ToSpace, Type::Int),
            ],
            exec: false,
            outputs: vec![CallOutput { field: None, port: WirePort::Output, ty: Type::Color }],
            receiver: Some(Type::Color),
        },
    );
}
