//! Messaging calls: chat, message boxes, and audio playback.

use super::*;

/// A player dialog: `(target, message, title?)`, answering with a record of
/// `ClickedBy` (the player who answered, first, so a bare use reads it), any
/// `extra` value, the exec field for the confirming button, and `Declined`.
fn dialog(
    name: &'static str,
    gate_class: &'static str,
    confirm: &str,
    extra: Option<(&str, Type)>,
) -> CallSpec {
    let mut fields = vec![("ClickedBy".to_string(), Type::Controller)];
    fields.extend(extra.map(|(f, t)| (f.to_string(), t)));
    fields.push((confirm.to_string(), Type::Exec));
    fields.push(("Declined".to_string(), Type::Exec));
    controller_exec(
        name,
        gate_class,
        vec![
            CallParam::req("target", WirePort::PlayerState, Type::Controller),
            CallParam::req("message", WirePort::Message, Type::Any),
            CallParam::opt("title", WirePort::Title, Type::Any),
        ],
        vec![CallOutput::plain(WirePort::ClickedBy, Type::Record(fields))],
    )
}

pub(super) fn register(m: &mut HashMap<&'static str, CallSpec>) {
    // ---- Messaging --------------------------------------------
    m.insert(
        "ShowChatMessage",
        controller_exec(
            "ShowChatMessage",
            gc::PLAYERSTATE_SHOW_CHAT,
            vec![
                CallParam::req("target", WirePort::PlayerState, Type::Controller),
                CallParam::req("message", WirePort::Message, Type::Any),
            ],
            vec![],
        ),
    );
    // The message box and the three question dialogs answer asynchronously:
    // `ExecOut` continues at once, and some ticks later the confirming button
    // pulses its exec output (`on r.YesClicked { ... }`), with `ClickedBy` the
    // player who answered. Every other outcome (cancel, no, closed, or a dialog
    // the game could not show) pulses `Declined` (`on r.Declined { ... }`).
    m.insert(
        "ShowMessageBox",
        dialog(
            "ShowMessageBox",
            gc::PLAYERSTATE_SHOW_MESSAGE_BOX,
            "OkClicked",
            None,
        ),
    );
    m.insert(
        "ShowOkCancelBox",
        dialog(
            "ShowOkCancelBox",
            gc::PLAYERSTATE_SHOW_OK_CANCEL_BOX,
            "OkClicked",
            None,
        ),
    );
    m.insert(
        "ShowYesNoBox",
        dialog(
            "ShowYesNoBox",
            gc::PLAYERSTATE_SHOW_YES_NO_BOX,
            "YesClicked",
            None,
        ),
    );
    m.insert(
        "ShowTextInputBox",
        dialog(
            "ShowTextInputBox",
            gc::PLAYERSTATE_SHOW_TEXT_INPUT_BOX,
            "OkClicked",
            Some(("Text", Type::String)),
        ),
    );
    m.insert(
        "BroadcastChatMessage",
        CallSpec {
            name: "BroadcastChatMessage",
            gate_class: gc::GAMEMODE_BROADCAST_CHAT,
            params: vec![CallParam::req("message", WirePort::Message, Type::Any)],
            exec: true,
            outputs: vec![],
            receiver: None,
        },
    );
    m.insert(
        "BroadcastStatusMessage",
        CallSpec {
            name: "BroadcastStatusMessage",
            gate_class: gc::GAMEMODE_BROADCAST_STATUS,
            params: vec![
                CallParam::req("message", WirePort::Message, Type::Any),
                CallParam::opt("flash", WirePort::BFlashIfUnchanged, Type::Bool),
            ],
            exec: true,
            outputs: vec![],
            receiver: None,
        },
    );

    // ---- Audio -------------------------------------------------------------
    // The audio asset is a `$BrickOneShotAudioDescriptor/...` reference,
    // inlined into the gate's AudioDescriptor data field (like GiveWeapon).
    m.insert(
        "PlayAudioAt",
        entity_exec(
            "PlayAudioAt",
            gc::PLAY_AUDIO_AT,
            vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("audio", WirePort::AudioDescriptor, Type::Entity),
                CallParam::opt("volume", WirePort::VolumeMultiplier, Type::Float),
                CallParam::opt("pitch", WirePort::PitchMultiplier, Type::Float),
                CallParam::opt("innerRadius", WirePort::InnerRadius, Type::Float),
                CallParam::opt("maxDistance", WirePort::MaxDistance, Type::Float),
                CallParam::opt("spatialized", WirePort::BSpatialization, Type::Bool),
            ],
            vec![],
        ),
    );
    m.insert(
        "PlayGlobalAudio",
        CallSpec {
            name: "PlayGlobalAudio",
            gate_class: gc::PLAY_GLOBAL_AUDIO,
            params: vec![
                CallParam::req("audio", WirePort::AudioDescriptor, Type::Entity),
                CallParam::opt("volume", WirePort::VolumeMultiplier, Type::Float),
                CallParam::opt("pitch", WirePort::PitchMultiplier, Type::Float),
            ],
            exec: true,
            outputs: vec![],
            receiver: None,
        },
    );
    // Non-spatial audio played to a single player (accepts a player character or
    // persistent player reference). `player` is a wired entity input; the audio
    // descriptor inlines into the AudioDescriptor config like the other Play* gates.
    m.insert(
        "PlayClientAudio",
        entity_exec(
            "PlayClientAudio",
            gc::PLAY_CLIENT_AUDIO,
            vec![
                CallParam::req("player", WirePort::Player, Type::Entity),
                CallParam::req("audio", WirePort::AudioDescriptor, Type::Entity),
                CallParam::opt("volume", WirePort::VolumeMultiplier, Type::Float),
                CallParam::opt("pitch", WirePort::PitchMultiplier, Type::Float),
            ],
            vec![],
        ),
    );
}
