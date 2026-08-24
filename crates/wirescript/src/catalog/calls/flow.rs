//! Control-flow and utility calls: sleeps, selects, detectors, timing, and misc gates.

use super::*;

pub(super) fn register(m: &mut HashMap<&'static str, CallSpec>) {
    // ---- Sleep (pure, delayed pass-through) ------------------------------
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
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Param("T".into()),
            }],
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
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Param("T".into()),
            }],
            receiver: None,
        },
    );

    // ---- Stateful exec value gates -------------------------
    // Advance per exec pulse: Cycle returns 0..Count-1, Toggle flips a bool.
    m.insert(
        "Cycle",
        CallSpec {
            name: "Cycle",
            gate_class: gc::EXEC_CYCLE,
            params: vec![CallParam::req("count", WirePort::Count, Type::Int)],
            exec: true,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Int,
            }],
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
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Bool,
            }],
            receiver: None,
        },
    );

    // ---- Random number ------------------------------------------------
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
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Int,
            }],
            receiver: None,
        },
    );

    // ---- Select / Swap --------------------------------------------------
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
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Param("T".into()),
            }],
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
            outputs: vec![CallOutput {
                field: None,
                port: WirePort::Output,
                ty: Type::Record(vec![
                    ("Output".into(), Type::Param("T".into())),
                    ("OutputB".into(), Type::Param("T".into())),
                ]),
            }],
            receiver: None,
        },
    );

    // ---- Edge detector ---------------------------------------------------
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

    // ---- Misc value / exec gates -----------------------------------------
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
    // Opaque — identity rerouter. Blocks constant folding: its output is
    // permanently Unknown to the (future) fold pass, and typecheck treats
    // the result as `any`, so probe circuits can drive real gates with
    // known values (`Opaque(2) + 3` emits a real MathAdd).
    m.insert(
        "Opaque",
        CallSpec {
            name: "Opaque",
            gate_class: gc::REROUTER,
            params: vec![CallParam::req("value", WirePort::RerInput, Type::Any)],
            exec: false,
            outputs: vec![CallOutput {
                field: None,
                port: WirePort::RerOutput,
                ty: Type::Opaque,
            }],
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
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::DeltaTime,
                ty: Type::Float,
            }],
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
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Uptime,
                ty: Type::Float,
            }],
            receiver: None,
        },
    );
    // Exec-flow gates callable directly. `Union(a, b)` merges two exec signals
    // into one (fires when EITHER fires) — the callable form of the gate the
    // compiler auto-inserts to rejoin exec paths. `Branch(cond, exec)` routes an
    // exec to `.A` or `.B` on `cond` (the gate an `if` lowers to). Both are pure
    // (no surrounding exec chain): their inputs ARE the exec signals they act on.
    //
    // `Union` also takes an exec receiver, so `a.Union(b)` == `Union(a, b)` and a
    // wide fan-in reads as a left-associative chain
    // (`a.Union(b).Union(c)`) instead of nested `Union(Union(a, b), c)`.
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
            vec![("A", WirePort::ExecOutA, Type::Exec), ("B", WirePort::ExecOutB, Type::Exec)],
        ),
    );
    // Read Brick Grid — the brick grid this gate's microchip is placed on.
    m.insert(
        "ReadBrickGrid",
        CallSpec {
            name: "ReadBrickGrid",
            gate_class: gc::READ_BRICK_GRID,
            params: vec![],
            exec: false,
            outputs: vec![CallOutput {
                field: None,
                port: WirePort::BrickGrid,
                ty: Type::Entity,
            }],
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
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::BOutput,
                ty: Type::Bool,
            }],
            receiver: None,
        },
    );
    m.insert(
        "Dampen",
        CallSpec {
            name: "Dampen",
            gate_class: gc::PSEUDO_DAMPEN,
            params: vec![
                CallParam::req("target", WirePort::Target, Type::Float),
                CallParam::req("smoothTime", WirePort::SmoothTime, Type::Float),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Value,
                ty: Type::Float,
            }],
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
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: blend_variant(),
            }],
            receiver: None,
        },
    );
    // Tween: stateful eased interpolation toward `target` over `duration`.
    m.insert(
        "Tween",
        CallSpec {
            name: "Tween",
            gate_class: gc::PSEUDO_TWEEN,
            params: vec![
                CallParam::req("target", WirePort::Target, blend_variant()),
                CallParam::req("duration", WirePort::Duration, Type::Float),
                CallParam::opt("function", WirePort::Function, Type::Any),
                CallParam::opt("direction", WirePort::Direction, Type::Any),
            ],
            exec: false,
            // Auto-unwraps to the interpolated `Value`; `.Arrived` pulses when it
            // reaches the target.
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Value,
                ty: Type::Record(vec![
                    ("Value".into(), blend_variant()),
                    ("Arrived".into(), Type::Exec),
                ]),
            }],
            receiver: None,
        },
    );
    // Timer: stateful countdown. `restart`/`pause`/`resume` are optional exec
    // signals that drive it; returns `{ Time: float, Expired: exec }`.
    m.insert(
        "Timer",
        CallSpec {
            name: "Timer",
            gate_class: gc::PSEUDO_TIMER,
            params: vec![
                CallParam::req("limit", WirePort::Limit, Type::Float),
                CallParam::opt("restart", WirePort::Restart, Type::Exec),
                CallParam::opt("pause", WirePort::Pause, Type::Exec),
                CallParam::opt("resume", WirePort::Resume, Type::Exec),
            ],
            exec: false,
            outputs: vec![
                CallOutput {
            field: None,
                    port: WirePort::Time,
                    ty: Type::Float,
                },
                CallOutput {
            field: None,
                    port: WirePort::Expired,
                    ty: Type::Exec,
                },
            ],
            receiver: None,
        },
    );

    // ---- Change detectors ----------------------------------------
    // `Change` pulses its input value through when it changes — that's the
    // "Change Detector (Exec)" gate (its OnChanged output moved there when
    // the game split the detectors into exec and bool variants). The plain "Change Detector"
    // emits a bool pulse instead, exposed as `Changed`.
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

    // ---- Pure expression gates --------------------------------------------
    m.insert(
        "EnumToInteger",
        CallSpec {
            name: "EnumToInteger",
            gate_class: gc::EXPR_ENUM_TO_INTEGER,
            params: vec![CallParam::req("value", WirePort::Input, Type::Any)],
            exec: false,
            outputs: vec![CallOutput { field: None, port: WirePort::Output, ty: Type::Int }],
            receiver: None,
        },
    );
    m.insert(
        "IntegerToEnum",
        CallSpec {
            name: "IntegerToEnum",
            gate_class: gc::EXPR_INTEGER_TO_ENUM,
            params: vec![
                CallParam::req("value", WirePort::Input, Type::Int),
                CallParam::opt("wrap", WirePort::BWrap, Type::Bool),
            ],
            exec: false,
            outputs: vec![CallOutput { field: None, port: WirePort::Output, ty: Type::Int }],
            receiver: None,
        },
    );

    // ---- Date / time -----------------------------------------------------
    m.insert(
        "GetUnixTime",
        CallSpec {
            name: "GetUnixTime",
            gate_class: gc::GET_UNIX_EPOCH,
            params: vec![],
            exec: false,
            outputs: vec![CallOutput { field: None, port: WirePort::UnixEpoch, ty: Type::Int }],
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
            outputs: vec![CallOutput {
                field: None,
                port: WirePort::Output,
                ty: Type::Record(vec![
                    ("Output".into(), Type::String),
                    ("Success".into(), Type::Bool),
                ]),
            }],
            receiver: None,
        },
    );
}
