//! PlayerState and character calls: UI, identity, permissions, and damage.

use super::*;

pub(super) fn register(m: &mut HashMap<&'static str, CallSpec>) {
    // ---- PlayerState --------------------------------------------------------
    // DisplayText's position/anchor/scale are composite `Position`/`Anchor`/
    // `Scale` struct ports, which the call form can't feed directly, so each
    // axis is exposed as its own float param, alongside per-axis styling
    // (colors, spacing, skew, wrap, z-order). `fontSize`/`justify`/`easing`
    // are constant-only data fields. `target` is the entity-typed
    // `PlayerState` port — a `controller` wires straight in.
    m.insert(
        "DisplayText",
        CallSpec {
            name: "DisplayText",
            gate_class: gc::PLAYERSTATE_DISPLAY_TEXT,
            params: vec![
                CallParam::req("target", WirePort::PlayerState, Type::Controller),
                CallParam::req("text", WirePort::Text, Type::String),
                // 2D layout ports are Vector2D composites (X/Y sub-ports): set
                // each axis with a float. A constant axis bakes the parent
                // Vector2D data field (unset axis from STRUCT_DEFAULTS); a
                // runtime value wires the `Position.X` / `Position.Y` sub-port.
                // (Assembly + wiring in lower/call.rs.)
                CallParam::opt("positionX", WirePort::PositionX, Type::Float),
                CallParam::opt("positionY", WirePort::PositionY, Type::Float),
                CallParam::opt("anchorX", WirePort::AnchorX, Type::Float),
                CallParam::opt("anchorY", WirePort::AnchorY, Type::Float),
                CallParam::opt("scaleX", WirePort::ScaleX, Type::Float),
                CallParam::opt("scaleY", WirePort::ScaleY, Type::Float),
                CallParam::opt("pivotX", WirePort::PivotX, Type::Float),
                CallParam::opt("pivotY", WirePort::PivotY, Type::Float),
                CallParam::opt("shadowOffsetX", WirePort::ShadowOffsetX, Type::Float),
                CallParam::opt("shadowOffsetY", WirePort::ShadowOffsetY, Type::Float),
                CallParam::opt("angle", WirePort::Angle, Type::Float),
                CallParam::opt("outlineSize", WirePort::OutlineSize, Type::Int),
                CallParam::opt("outlineColor", WirePort::OutlineColor, Type::Color),
                CallParam::opt("fontColor", WirePort::FontColor, Type::Color),
                CallParam::opt("shadowColor", WirePort::ShadowColor, Type::Color),
                CallParam::opt("miteredOutline", WirePort::BMiteredOutline, Type::Bool),
                CallParam::opt("letterSpacing", WirePort::LetterSpacing, Type::Float),
                CallParam::opt("lineHeight", WirePort::LineHeight, Type::Float),
                CallParam::opt("wrapWidth", WirePort::WrapWidth, Type::Float),
                CallParam::opt("skew", WirePort::Skew, Type::Float),
                CallParam::opt("zOrder", WirePort::ZOrder, Type::Int),
                CallParam::opt("lifetime", WirePort::Lifetime, Type::Float),
                CallParam::opt("transition", WirePort::Transition, Type::Float),
                CallParam::opt("textId", WirePort::TextId, Type::Int),
                // Constant-only data fields (not wire inputs) — see DATA_ONLY_PARAMS.
                CallParam::opt("fontSize", WirePort::FontSize, Type::Int),
                CallParam::opt("justify", WirePort::Justification, Type::Int),
                CallParam::opt("easing", WirePort::Easing, Type::Int),
                // `typeface` is an EBRTextTypeface enum member; `font` is a
                // font asset ref (`$BrickFontDescriptor/…`) — an object reference (typed
                // `entity`, like every asset ref).
                CallParam::opt("typeface", WirePort::Typeface, Type::Int),
                CallParam::opt("font", WirePort::Font, Type::Entity),
            ],
            exec: true,
            // The gate echoes the (resolved) text id, so a later call can update
            // or clear the same on-screen text: `let id = p.DisplayText(...)`.
            outputs: vec![CallOutput {
                field: None,
                port: WirePort::TextIdOut,
                ty: Type::Int,
            }],
            receiver: Some(Type::Controller),
        },
    );

    // ---- Character / Controller conversions ------------------------------
    // `ControllerOf` lowers to `PlayerState_GetFromEntity` ("Get Player
    // (Persistent)"), whose player output is the entity-typed `PlayerState`.
    m.insert(
        "ControllerOf",
        entity_exec(
            "ControllerOf",
            gc::PLAYERSTATE_GET_FROM_ENTITY,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput {
            field: None,
                port: WirePort::PlayerState,
                ty: Type::Controller,
            }],
        ),
    );
    // `CharacterOf` uses the `Character_GetFromController` gate; its player
    // input port is the entity-typed `PlayerState`.
    m.insert(
        "CharacterOf",
        controller_exec(
            "CharacterOf",
            gc::CHARACTER_GET_FROM_CONTROLLER,
            vec![CallParam::req("controller", WirePort::PlayerState, Type::Controller)],
            vec![CallOutput {
            field: None,
                port: WirePort::Character,
                ty: Type::Character,
            }],
        ),
    );

    // ---- Camera / aim ---------------------------------------------------
    // Single GetAim gate exposing both outputs as a record:
    // `char.GetAim().Origin` / `.Direction`.
    m.insert(
        "GetAim",
        character_exec(
            "GetAim",
            gc::CHARACTER_GET_AIM,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                // Config-only (settings menu, not a wire input).
                CallParam::opt("localAim", WirePort::BLocalAim, Type::Bool),
            ],
            vec![CallOutput {
            field: None,
                port: WirePort::Origin,
                ty: Type::Record(vec![
                    ("Origin".into(), Type::Vector),
                    ("Direction".into(), Type::Vector),
                ]),
            }],
        ),
    );
    m.insert(
        "InputReader",
        CallSpec {
            name: "InputReader",
            gate_class: gc::INPUT_SPLITTER,
            params: vec![CallParam::req("character", WirePort::Character, Type::Character)],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::InputForward,
                ty: Type::Record(vec![
                    ("Forward".into(), Type::Float),
                    ("Right".into(), Type::Float),
                    ("Up".into(), Type::Float),
                    ("Pitch".into(), Type::Float),
                    ("Yaw".into(), Type::Float),
                    ("Roll".into(), Type::Float),
                    ("MouseWheel".into(), Type::Float),
                    ("PressedC".into(), Type::Bool),
                    ("PressedE".into(), Type::Bool),
                    ("PressedQ".into(), Type::Bool),
                    ("PressedLeftMouse".into(), Type::Bool),
                    ("PressedRightMouse".into(), Type::Bool),
                ]),
            }],
            receiver: Some(Type::Character),
        },
    );
    // The exec-form counterpart of `InputReader`: same twelve controls, sampled
    // once when the exec chain reaches it instead of read continuously. The
    // field names are deliberately identical to the splitter's, so the two read
    // the same at a call site and only the context differs. Its operand port is
    // `Player` rather than `Character` (the gate accepts a character or a
    // persistent player), while the surface type stays `Character`: a
    // controller wires straight into a character param, so the permissive port
    // costs no expressiveness here.
    m.insert(
        "GetInputs",
        character_exec(
            "GetInputs",
            gc::EXEC_GET_INPUTS,
            vec![CallParam::req("player", WirePort::Player, Type::Character)],
            vec![CallOutput {
                field: None,
                port: WirePort::InputForward,
                ty: Type::Record(vec![
                    ("Forward".into(), Type::Float),
                    ("Right".into(), Type::Float),
                    ("Up".into(), Type::Float),
                    ("Pitch".into(), Type::Float),
                    ("Yaw".into(), Type::Float),
                    ("Roll".into(), Type::Float),
                    ("MouseWheel".into(), Type::Float),
                    ("PressedC".into(), Type::Bool),
                    ("PressedE".into(), Type::Bool),
                    ("PressedQ".into(), Type::Bool),
                    ("PressedLeftMouse".into(), Type::Bool),
                    ("PressedRightMouse".into(), Type::Bool),
                ]),
            }],
        ),
    );

    // ---- PlayerState role check -----------------------------
    // `ctrl.HasRole("Admin")` — RoleName is a config string, returns a bool.
    m.insert(
        "HasRole",
        controller_exec(
            "HasRole",
            gc::PLAYERSTATE_HAS_ROLE,
            vec![
                CallParam::req("target", WirePort::PlayerState, Type::Controller),
                CallParam::req("role", WirePort::RoleName, Type::String),
            ],
            vec![CallOutput {
            field: None,
                port: WirePort::BHasRole,
                ty: Type::Bool,
            }],
        ),
    );

    // ---- Character (additional) ------------------------------------------
    m.insert(
        "ShowHint",
        character_exec(
            "ShowHint",
            gc::CHARACTER_SHOW_HINT,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("title", WirePort::HintTitle, Type::String),
                CallParam::req("text", WirePort::HintText, Type::String),
            ],
            vec![],
        ),
    );

    m.insert(
        "GetDamage",
        character_exec(
            "GetDamage",
            gc::CHARACTER_GET_DAMAGE,
            vec![CallParam::req("character", WirePort::Character, Type::Character)],
            vec![CallOutput {
            field: None,
                port: WirePort::Damage,
                ty: Type::Record(vec![
                    ("Damage".into(), Type::Float),
                    ("DamageLimit".into(), Type::Float),
                ]),
            }],
        ),
    );
    m.insert(
        "SetDamage",
        character_exec(
            "SetDamage",
            gc::CHARACTER_SET_DAMAGE,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("damage", WirePort::Damage, Type::Float),
            ],
            vec![],
        ),
    );
    m.insert(
        "IncDamage",
        character_exec(
            "IncDamage",
            gc::CHARACTER_INC_DAMAGE,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("amount", WirePort::Amount, Type::Float),
            ],
            vec![],
        ),
    );

    // ---- PlayerState (additional) -----------------------------------------
    m.insert(
        "ShowStatusMessage",
        controller_exec(
            "ShowStatusMessage",
            gc::PLAYERSTATE_SHOW_STATUS,
            vec![
                CallParam::req("controller", WirePort::PlayerState, Type::Controller),
                CallParam::req("message", WirePort::Message, Type::String),
            ],
            vec![],
        ),
    );
    m.insert(
        "GetUserName",
        controller_exec(
            "GetUserName",
            gc::PLAYERSTATE_GET_USER_NAME,
            vec![CallParam::req("controller", WirePort::PlayerState, Type::Controller)],
            vec![CallOutput {
            field: None,
                port: WirePort::UserName,
                ty: Type::String,
            }],
        ),
    );
    m.insert(
        "GetUserId",
        controller_exec(
            "GetUserId",
            gc::PLAYERSTATE_GET_USER_ID,
            vec![CallParam::req("controller", WirePort::PlayerState, Type::Controller)],
            vec![CallOutput {
            field: None,
                port: WirePort::UserId,
                ty: Type::String,
            }],
        ),
    );
    m.insert(
        "GetDisplayName",
        controller_exec(
            "GetDisplayName",
            gc::PLAYERSTATE_GET_DISPLAY_NAME,
            vec![CallParam::req("controller", WirePort::PlayerState, Type::Controller)],
            vec![CallOutput {
            field: None,
                port: WirePort::DisplayName,
                ty: Type::String,
            }],
        ),
    );
    m.insert(
        "IsTrusted",
        controller_exec(
            "IsTrusted",
            gc::PLAYERSTATE_IS_TRUSTED,
            vec![CallParam::req("controller", WirePort::PlayerState, Type::Controller)],
            vec![CallOutput {
            field: None,
                port: WirePort::BIsTrusted,
                ty: Type::Bool,
            }],
        ),
    );
    m.insert(
        "SetCanRespawn",
        controller_exec(
            "SetCanRespawn",
            gc::PLAYERSTATE_SET_CAN_RESPAWN,
            vec![
                CallParam::req("controller", WirePort::PlayerState, Type::Controller),
                CallParam::req("canRespawn", WirePort::BCanRespawn, Type::Bool),
            ],
            vec![],
        ),
    );
    // Force Respawn Player — immediately respawns the player. Exec, no outputs
    // beyond ExecOut; the gate takes only the persistent player-state ref.
    m.insert(
        "ForceRespawn",
        controller_exec(
            "ForceRespawn",
            gc::PLAYERSTATE_FORCE_RESPAWN,
            vec![CallParam::req("controller", WirePort::PlayerState, Type::Controller)],
            vec![],
        ),
    );
    m.insert(
        "HasPermission",
        controller_exec(
            "HasPermission",
            gc::PLAYERSTATE_HAS_PERMISSION,
            vec![
                CallParam::req("controller", WirePort::PlayerState, Type::Controller),
                CallParam::req("permission", WirePort::PermissionName, Type::String),
            ],
            vec![CallOutput {
            field: None,
                port: WirePort::BHasPermission,
                ty: Type::Bool,
            }],
        ),
    );
    m.insert(
        "SetTempPermission",
        character_exec(
            "SetTempPermission",
            gc::CHARACTER_SET_TEMP_PERMISSION,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("permission", WirePort::PermissionTagStr, Type::String),
                CallParam::req("enable", WirePort::BPermissionEnable, Type::Bool),
            ],
            vec![],
        ),
    );
    m.insert(
        "SetTeamPinned",
        controller_exec(
            "SetTeamPinned",
            gc::GAMEMODE_SET_TEAM_PINNED,
            vec![
                CallParam::req("controller", WirePort::PlayerState, Type::Controller),
                CallParam::req("pinned", WirePort::BPinned, Type::Bool),
            ],
            vec![],
        ),
    );
}
