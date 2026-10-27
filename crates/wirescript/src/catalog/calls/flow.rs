//! Control-flow and utility calls: sleeps, selects, detectors, timing, and misc gates.

use super::*;

pub(super) fn register(m: &mut HashMap<&'static str, CallSpec>) {
    // Sleep (pure, delayed pass-through)
    m.insert(
        "Sleep",
        CallSpec {
            name: "Sleep",
            gate_class: gc::BUFFER_SECONDS,
            params: vec![
                CallParam::req("input", WirePort::Input, Type::Param("T".into())),
                CallParam::opt("delay", WirePort::SecondsToWait, Type::Float),
                CallParam::opt("hold", WirePort::ZeroSecondsToWait, Type::Float),
            ],
            exec: false,
            // Passthrough: the delayed output carries the input's type.
            outputs: vec![CallOutput::plain(WirePort::Output, Type::Param("T".into()))],
            receiver: None,
        },
    );
    m.insert(
        "SleepTicks",
        CallSpec {
            name: "SleepTicks",
            gate_class: gc::BUFFER_TICKS,
            params: vec![
                CallParam::req("input", WirePort::Input, Type::Param("T".into())),
                CallParam::opt("delay", WirePort::TicksToWait, Type::Int),
                CallParam::opt("hold", WirePort::ZeroTicksToWait, Type::Int),
            ],
            exec: false,
            // Passthrough: the delayed output carries the input's type.
            outputs: vec![CallOutput::plain(WirePort::Output, Type::Param("T".into()))],
            receiver: None,
        },
    );

    // Stateful exec value gates
    // Advance per exec pulse: Cycle returns 0..Count-1, Toggle flips a bool.
    m.insert(
        "Cycle",
        CallSpec {
            name: "Cycle",
            gate_class: gc::EXEC_CYCLE,
            params: vec![CallParam::req("count", WirePort::Count, Type::Int)],
            exec: true,
            outputs: vec![CallOutput::plain(WirePort::Output, Type::Int)],
            receiver: None,
        },
    );
    m.insert(
        "Toggle",
        CallSpec {
            name: "Toggle",
            gate_class: gc::EXEC_TOGGLE,
            params: vec![],
            exec: true,
            outputs: vec![CallOutput::plain(WirePort::Output, Type::Bool)],
            receiver: None,
        },
    );

    // Random number
    m.insert(
        "Random",
        CallSpec {
            name: "Random",
            gate_class: gc::RANDOM,
            params: vec![
                CallParam::req("min", WirePort::Min, Type::Int),
                CallParam::req("max", WirePort::Max, Type::Int),
            ],
            exec: true,
            outputs: vec![CallOutput::plain(WirePort::Output, Type::Int)],
            receiver: None,
        },
    );

    // Select / Swap
    m.insert(
        "Select",
        CallSpec {
            name: "Select",
            gate_class: gc::SELECT,
            params: vec![
                CallParam::req("cond", WirePort::BSelectB, Type::Bool),
                CallParam::req("a", WirePort::InputA, Type::Param("T".into())),
                CallParam::req("b", WirePort::InputB, Type::Param("T".into())),
            ],
            exec: false,
            // Generic: picks `a` or `b`, so the result carries their shared type.
            outputs: vec![CallOutput::plain(WirePort::Output, Type::Param("T".into()))],
            receiver: None,
        },
    );
    m.insert(
        "Swap",
        CallSpec {
            name: "Swap",
            gate_class: gc::SWAP,
            params: vec![
                CallParam::req("cond", WirePort::BSwap, Type::Bool),
                CallParam::req("a", WirePort::InputA, Type::Param("T".into())),
                CallParam::req("b", WirePort::InputB, Type::Param("T".into())),
            ],
            exec: false,
            // Generic: auto-unwraps to the first (possibly swapped) value;
            // `.OutputB` is the other one. Both carry `a`/`b`'s shared type.
            outputs: vec![CallOutput::plain(
                WirePort::Output,
                Type::Record(vec![
                    ("Output".into(), Type::Param("T".into())),
                    ("OutputB".into(), Type::Param("T".into())),
                ]),
            )],
            receiver: None,
        },
    );

    // Edge detector
    m.insert(
        "Edge",
        vec_expr_record(
            "Edge",
            gc::EDGE_DETECTOR,
            vec![CallParam::req("input", WirePort::Input, Type::Bool)],
            vec![
                ("Rising", WirePort::BPulseOnRisingEdge, Type::Bool),
                ("Falling", WirePort::BPulseOnFallingEdge, Type::Bool),
            ],
        ),
    );
    // "Edge Detector (Exec)": exec pulses when a float input rises/falls,
    // for `on e.Rising { }` / `await e.Falling` (Timer.Expired-style).
    m.insert(
        "EdgeExec",
        vec_expr_record(
            "EdgeExec",
            gc::EDGE_DETECTOR_EXEC,
            vec![CallParam::req("input", WirePort::Input, Type::Float)],
            vec![
                ("Rising", WirePort::OnRisingEdge, Type::Exec),
                ("Falling", WirePort::OnFallingEdge, Type::Exec),
            ],
        ),
    );

    // Misc value / exec gates
    m.insert(
        "PrintToConsole",
        CallSpec {
            name: "PrintToConsole",
            gate_class: gc::PRINT_TO_CONSOLE,
            params: vec![CallParam::req("text", WirePort::Text, Type::String)],
            exec: true,
            outputs: vec![],
            receiver: None,
        },
    );
    // Opaque: an identity rerouter that constant folding never sees through,
    // so probe circuits drive real gates with known values (`Opaque(2) + 3`
    // emits a real MathAdd).
    m.insert(
        "Opaque",
        CallSpec {
            name: "Opaque",
            gate_class: gc::REROUTER,
            params: vec![CallParam::req("value", WirePort::RerInput, Type::Any)],
            exec: false,
            outputs: vec![CallOutput::plain(WirePort::RerOutput, Type::Opaque)],
            receiver: None,
        },
    );
    m.insert(
        "DeltaTime",
        CallSpec {
            name: "DeltaTime",
            gate_class: gc::DELTA_TIME,
            params: vec![],
            exec: false,
            outputs: vec![CallOutput::plain(WirePort::DeltaTime, Type::Float)],
            receiver: None,
        },
    );
    m.insert(
        "ServerUptime",
        CallSpec {
            name: "ServerUptime",
            gate_class: gc::SERVER_UPTIME,
            params: vec![],
            exec: false,
            outputs: vec![CallOutput::plain(WirePort::Uptime, Type::Float)],
            receiver: None,
        },
    );
    // Exec-flow gates as calls. `Union(a, b)` fires when either exec fires;
    // `Branch(cond, exec)` routes an exec to `.A` or `.B`. Both are pure: their
    // inputs are the exec signals they act on. `a.Union(b).Union(c)` chains.
    m.insert("Union", {
        let mut union = vec_expr(
            "Union",
            gc::UNION,
            vec![
                CallParam::req("a", WirePort::ExecA, Type::Exec),
                CallParam::req("b", WirePort::ExecB, Type::Exec),
            ],
            WirePort::ExecOut,
            Type::Exec,
        );
        union.receiver = Some(Type::Exec);
        union
    });
    m.insert(
        "Branch",
        vec_expr_record(
            "Branch",
            gc::BRANCH,
            vec![
                CallParam::req("cond", WirePort::BCond, Type::Bool),
                CallParam::req("exec", WirePort::Exec, Type::Exec),
            ],
            vec![
                ("A", WirePort::ExecOutA, Type::Exec),
                ("B", WirePort::ExecOutB, Type::Exec),
            ],
        ),
    );
    // Read Brick Grid: the brick grid this gate's microchip is placed on.
    m.insert(
        "ReadBrickGrid",
        CallSpec {
            name: "ReadBrickGrid",
            gate_class: gc::READ_BRICK_GRID,
            params: vec![],
            exec: false,
            outputs: vec![CallOutput::plain(WirePort::BrickGrid, Type::Entity)],
            receiver: None,
        },
    );
    m.insert(
        "NearlyEqual",
        CallSpec {
            name: "NearlyEqual",
            gate_class: gc::NEARLY_EQUAL,
            params: vec![
                CallParam::req("a", WirePort::InputA, Type::Float),
                CallParam::req("b", WirePort::InputB, Type::Float),
                CallParam::req("tolerance", WirePort::Tolerance, Type::Float),
            ],
            exec: false,
            outputs: vec![CallOutput::plain(WirePort::BOutput, Type::Bool)],
            receiver: None,
        },
    );
    m.insert(
        "Dampen",
        CallSpec {
            name: "Dampen",
            gate_class: gc::DAMPEN,
            params: vec![
                CallParam::req("target", WirePort::Target, blend_variant()),
                CallParam::req("smoothTime", WirePort::SmoothTime, Type::Float),
            ],
            exec: false,
            outputs: vec![CallOutput::plain(WirePort::Value, blend_variant())],
            receiver: None,
        },
    );
    // Easing: interpolate a..b by blend with an easing curve. `function` and
    // `direction` accept an int or an easing-enum name literal ("Quad",
    // "InOut", ...) resolved against EBREasingFunction / EBREasingDirection.
    m.insert(
        "Easing",
        CallSpec {
            name: "Easing",
            gate_class: gc::MATH_EASING,
            params: vec![
                CallParam::req("a", WirePort::InputA, blend_variant()),
                CallParam::req("b", WirePort::InputB, blend_variant()),
                CallParam::req("blend", WirePort::Blend, Type::Float),
                CallParam::opt("function", WirePort::Function, Type::Any),
                CallParam::opt("direction", WirePort::Direction, Type::Any),
            ],
            exec: false,
            outputs: vec![CallOutput::plain(WirePort::Output, blend_variant())],
            receiver: None,
        },
    );
    // Tween: stateful eased interpolation toward `target` over `duration`.
    m.insert(
        "Tween",
        CallSpec {
            name: "Tween",
            gate_class: gc::TWEEN,
            params: vec![
                CallParam::req("target", WirePort::Target, blend_variant()),
                CallParam::req("duration", WirePort::Duration, Type::Float),
                CallParam::opt("function", WirePort::Function, Type::Any),
                CallParam::opt("direction", WirePort::Direction, Type::Any),
            ],
            exec: false,
            // Auto-unwraps to the interpolated `Value`; `.Arrived` pulses when it
            // reaches the target.
            outputs: vec![CallOutput::plain(
                WirePort::Value,
                Type::Record(vec![
                    ("Value".into(), blend_variant()),
                    ("Arrived".into(), Type::Exec),
                ]),
            )],
            receiver: None,
        },
    );
    // Timer: stateful countdown. `restart`/`pause`/`resume` are optional exec
    // signals that drive it; returns `{ Time: float, Expired: exec }`.
    m.insert(
        "Timer",
        CallSpec {
            name: "Timer",
            gate_class: gc::STOPWATCH,
            params: vec![
                CallParam::req("limit", WirePort::Limit, Type::Float),
                CallParam::opt("restart", WirePort::Restart, Type::Exec),
                CallParam::opt("pause", WirePort::Pause, Type::Exec),
                CallParam::opt("resume", WirePort::Resume, Type::Exec),
            ],
            exec: false,
            outputs: vec![
                CallOutput::plain(WirePort::Time, Type::Float),
                CallOutput::plain(WirePort::Expired, Type::Exec),
            ],
            receiver: None,
        },
    );

    // Change detectors
    // `Change` is the "Change Detector (Exec)" gate: an exec pulse carrying the
    // new value when its input changes. The plain "Change Detector" emits a bool
    // pulse instead, exposed as `Changed`.
    m.insert(
        "Change",
        vec_expr(
            "Change",
            gc::CHANGE_DETECTOR_EXEC,
            vec![CallParam::req("input", WirePort::Input, Type::Any)],
            WirePort::OnChanged,
            Type::Exec,
        ),
    );
    // A container's contents: pulses on the update after the array or map
    // changes in any way. `Change` watches a wire value, and a container's
    // wire carries the same reference however its contents move (WS059).
    m.insert(
        "ContainerChange",
        vec_expr(
            "ContainerChange",
            gc::CONTAINER_CHANGE_DETECTOR,
            vec![CallParam::req(
                "container",
                WirePort::ContainerRef,
                Type::Any,
            )],
            WirePort::OnChanged,
            Type::Exec,
        ),
    );
    m.insert(
        "Changed",
        vec_expr(
            "Changed",
            gc::CHANGE_DETECTOR,
            vec![CallParam::req("input", WirePort::Input, Type::Any)],
            WirePort::BPulseOnChange,
            Type::Bool,
        ),
    );

    // Pure expression gates

    // Which kind of value is on the wire, as the built-in enum `Type`
    // (`catalog::WIRE_KIND_MEMBERS`). `v.getType()` is the same gate as a method
    // of every value.
    let value_type = || Type::Enum {
        name: "Type".into(),
        args: vec![],
    };
    m.insert(
        "GetValueType",
        CallSpec {
            name: "GetValueType",
            gate_class: gc::GET_VALUE_TYPE,
            params: vec![CallParam::req("value", WirePort::Input, Type::Any)],
            exec: false,
            outputs: vec![CallOutput::plain(WirePort::Type, value_type())],
            receiver: None,
        },
    );
    m.insert(
        "getType",
        CallSpec {
            name: "getType",
            gate_class: gc::GET_VALUE_TYPE,
            params: vec![CallParam::req("value", WirePort::Input, Type::Any)],
            exec: false,
            outputs: vec![CallOutput::plain(WirePort::Type, value_type())],
            receiver: Some(Type::Any),
        },
    );
    // `EnumToInt`/`IntToEnum` are the gate forms of `value.ToInt()` and
    // `Enum.FromInt(n)`. These specs describe the raw gates; typecheck requires
    // an enum argument for `EnumToInt` and types `IntToEnum` from the use site,
    // and lowering folds a compile-time-known enum (`lower::expr`'s
    // `try_lower_enum_to_integer` / `try_lower_integer_to_enum`).
    m.insert(
        "EnumToInt",
        CallSpec {
            name: "EnumToInt",
            gate_class: gc::EXPR_ENUM_TO_INTEGER,
            params: vec![CallParam::req("value", WirePort::Input, Type::Any)],
            exec: false,
            outputs: vec![CallOutput::plain(WirePort::Output, Type::Int)],
            receiver: None,
        },
    );
    m.insert(
        "IntToEnum",
        CallSpec {
            name: "IntToEnum",
            gate_class: gc::EXPR_INTEGER_TO_ENUM,
            params: vec![
                CallParam::req("value", WirePort::Input, Type::Int),
                CallParam::opt("wrap", WirePort::BWrap, Type::Bool),
            ],
            exec: false,
            // The raw gate output is the numeric discriminant; the surface
            // result is the context-typed enum.
            outputs: vec![CallOutput::plain(WirePort::Output, Type::Int)],
            receiver: None,
        },
    );

    // Date / time
    m.insert(
        "GetUnixTime",
        CallSpec {
            name: "GetUnixTime",
            gate_class: gc::GET_UNIX_EPOCH,
            params: vec![],
            exec: false,
            outputs: vec![CallOutput::plain(WirePort::UnixEpoch, Type::Int)],
            receiver: None,
        },
    );
    m.insert(
        "FormatDate",
        CallSpec {
            name: "FormatDate",
            gate_class: gc::FORMAT_DATE,
            params: vec![
                CallParam::req("unixTime", WirePort::UnixEpoch, Type::Int),
                CallParam::req("format", WirePort::Format, Type::String),
                CallParam::opt("useUTC", WirePort::BUseUTC, Type::Bool),
            ],
            exec: false,
            outputs: vec![CallOutput::plain(
                WirePort::Output,
                Type::Record(vec![
                    ("Output".into(), Type::String),
                    ("Success".into(), Type::Bool),
                ]),
            )],
            receiver: None,
        },
    );
}
