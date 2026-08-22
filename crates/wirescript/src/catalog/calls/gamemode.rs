//! Gamemode calls: leaderboards, teams, and round/win state.

use super::*;

pub(super) fn register(m: &mut HashMap<&'static str, CallSpec>) {
    // ---- Gamemode -------------------------------------------------------
    m.insert(
        "SetLeaderboard",
        controller_exec(
            "SetLeaderboard",
            gc::GAMEMODE_SET_LEADERBOARD,
            vec![
                CallParam::req("controller", WirePort::PlayerState, Type::Controller),
                CallParam::req("key", WirePort::Key, Type::String),
                CallParam::req("value", WirePort::Value, Type::Int),
            ],
            vec![],
        ),
    );
    m.insert(
        "IncLeaderboard",
        controller_exec(
            "IncLeaderboard",
            gc::GAMEMODE_INC_LEADERBOARD,
            vec![
                CallParam::req("controller", WirePort::PlayerState, Type::Controller),
                CallParam::req("key", WirePort::Key, Type::String),
                CallParam::req("value", WirePort::Value, Type::Int),
            ],
            vec![],
        ),
    );
    m.insert(
        "GetLeaderboard",
        controller_exec(
            "GetLeaderboard",
            gc::GAMEMODE_GET_LEADERBOARD,
            vec![
                CallParam::req("controller", WirePort::PlayerState, Type::Controller),
                CallParam::req("key", WirePort::Key, Type::String),
            ],
            vec![CallOutput {
            field: None,
                port: WirePort::Value,
                ty: Type::Int,
            }],
        ),
    );
    m.insert(
        "GetTeam",
        character_exec(
            "GetTeam",
            gc::GAMEMODE_GET_TEAM,
            vec![CallParam::req("character", WirePort::Character, Type::Character)],
            vec![CallOutput {
            field: None,
                port: WirePort::Team,
                ty: Type::Entity,
            }],
        ),
    );

    // Team predicates (pure): `IsBuilderTeam(team)` / `team.IsBuilderTeam()` and
    // the unaffiliated-team counterpart. Take a Team entity, return a bool.
    m.insert(
        "IsBuilderTeam",
        CallSpec {
            name: "IsBuilderTeam",
            gate_class: gc::GAMEMODE_IS_BUILDER_TEAM,
            params: vec![CallParam::req("team", WirePort::Team, Type::Entity)],
            exec: false,
            outputs: vec![CallOutput {
                field: None,
                port: WirePort::BResult,
                ty: Type::Bool,
            }],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "IsUnaffiliatedTeam",
        CallSpec {
            name: "IsUnaffiliatedTeam",
            gate_class: gc::GAMEMODE_IS_UNAFFILIATED_TEAM,
            params: vec![CallParam::req("team", WirePort::Team, Type::Entity)],
            exec: false,
            outputs: vec![CallOutput {
                field: None,
                port: WirePort::BResult,
                ty: Type::Bool,
            }],
            receiver: Some(Type::Entity),
        },
    );

    // ---- Gamemode (additional) -------------------------------------------
    // A round ends by declaring a winner via PlayerWins / TeamWins.
    m.insert(
        "PlayerWins",
        CallSpec {
            name: "PlayerWins",
            gate_class: gc::GAMEMODE_PLAYER_WINS,
            params: vec![
                CallParam::req("player", WirePort::Player, Type::Controller),
                CallParam::opt("teamWinsInstead", WirePort::BTeamWinsInstead, Type::Bool),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Controller),
        },
    );
    m.insert(
        "TeamWins",
        CallSpec {
            name: "TeamWins",
            gate_class: gc::GAMEMODE_TEAM_WINS,
            params: vec![CallParam::req("team", WirePort::Team, Type::Entity)],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "GetCurrentRound",
        CallSpec {
            name: "GetCurrentRound",
            gate_class: gc::GAMEMODE_GET_CURRENT_ROUND,
            params: vec![],
            exec: true,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::RoundNumber,
                ty: Type::Int,
            }],
            receiver: None,
        },
    );
    m.insert(
        "GetTeamByName",
        CallSpec {
            name: "GetTeamByName",
            gate_class: gc::GAMEMODE_GET_TEAM_BY_NAME,
            params: vec![CallParam::req("name", WirePort::TeamName, Type::String)],
            exec: true,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Team,
                ty: Type::Entity,
            }],
            receiver: None,
        },
    );

    // ---- Gamemode teams (additional) -------------------------------------
    m.insert(
        "SetTeam",
        controller_exec(
            "SetTeam",
            gc::GAMEMODE_SET_TEAM,
            vec![
                CallParam::req("controller", WirePort::PlayerState, Type::Controller),
                CallParam::req("team", WirePort::Team, Type::Entity),
                CallParam::opt("pin", WirePort::BPinPlayerToTeam, Type::Bool),
            ],
            vec![],
        ),
    );
    m.insert(
        "GetTeamName",
        CallSpec {
            name: "GetTeamName",
            gate_class: gc::GAMEMODE_GET_TEAM_NAME,
            params: vec![CallParam::req("team", WirePort::Team, Type::Entity)],
            exec: true,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Name,
                ty: Type::String,
            }],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "GetTeamLeaderboardValue",
        CallSpec {
            name: "GetTeamLeaderboardValue",
            gate_class: gc::GAMEMODE_GET_TEAM_LEADERBOARD,
            params: vec![
                CallParam::req("team", WirePort::Team, Type::Entity),
                CallParam::req("key", WirePort::Key, Type::String),
            ],
            exec: true,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Value,
                ty: Type::Int,
            }],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "SetTeamLeaderboardValue",
        CallSpec {
            name: "SetTeamLeaderboardValue",
            gate_class: gc::GAMEMODE_SET_TEAM_LEADERBOARD,
            params: vec![
                CallParam::req("team", WirePort::Team, Type::Entity),
                CallParam::req("key", WirePort::Key, Type::String),
                CallParam::req("value", WirePort::Value, Type::Int),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "IncrementTeamLeaderboardValue",
        CallSpec {
            name: "IncrementTeamLeaderboardValue",
            gate_class: gc::GAMEMODE_INC_TEAM_LEADERBOARD,
            params: vec![
                CallParam::req("team", WirePort::Team, Type::Entity),
                CallParam::req("key", WirePort::Key, Type::String),
                CallParam::req("value", WirePort::Value, Type::Int),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Entity),
        },
    );
}
