//! A world a test scripts.
//!
//! A script is lines of `<tick> <action> ...`, `#` starting a comment:
//!
//! ```text
//! 5 join alice            # alice joins (ControllerJoined)
//! 10 chat alice /roll 3   # alice types /roll 3 (ChatCommand "roll", arguments "3")
//! 12 leave alice          # ControllerLeft
//! 20 round                # RoundStart
//! 30 roundend             # RoundEnd
//! 0 sweep 120.5           # the next sweep hits at that distance
//! 0 miss                  # the next sweep misses
//! 0 player alice name Sam the Builder   # alice's display name
//! 0 player alice user sam                # alice's user name
//! 0 player alice id 0b5a...              # alice's user id
//! 0 player alice permission Build        # alice has that permission
//! 0 player alice role Admin              # alice has that role
//! 0 player alice trusted yes             # alice is trusted by the brick owner
//! 0 player alice damagelimit 150         # alice's characters take that much damage
//! 0 team Red alice bob                   # team Red, with alice and bob on it
//! 0 teleport 0 0 500                     # where a teleport destination is
//! 7 move alice 10 0 -20                  # alice's character moves there
//! 9 input alice Forward 1                # alice holds forward (until set again)
//! 9 input alice E 1                      # alice holds E
//! 9 aim alice 0 1 0                      # alice aims along that direction
//! 9 slot alice 2                         # alice holds inventory slot 2
//! 9 zone alice bob                       # who stands in the zone (none: empty)
//! 11 spawn alice                         # CharacterSpawned
//! 12 damage alice 25 bob                 # CharacterDamaged (bob attacking)
//! 13 fire alice Pistol                   # CharacterFiredWeapon
//! 14 die alice bob                       # CharacterDied (bob the killer)
//! 15 jointeam alice Blue                 # ControllerLeftTeam, ControllerJoinedTeam
//! 16 leaveteam alice                     # ControllerLeftTeam
//! 17 interact alice                      # WholeGridInteracted (`interact alice held`)
//! 18 target alice 10 Bow                 # WholeGridTargeted
//! 19 answer alice yes                    # alice answers her oldest open box
//! ```
//!
//! Sweep lines queue results in file order whatever their tick. Player,
//! team and teleport lines set the world up before the run and take tick 0.
//! A player nothing names goes by [`DEFAULT_NAME`] and a user id made from
//! their script name. A `move`, `input`, `aim`, `slot` or `zone` at tick 0 is
//! how the run starts. An answer is `yes` or `ok`, `no` or `cancel`, or
//! `text <words>`.
//!
//! What the program shows one player is printed among the chat, prefixed with
//! the player's script name: `[alice text 3] Score: 5` when `DisplayText`
//! changes the text with id 3, `[alice chat] hi` for `ShowChatMessage`. Other
//! things a program does to the world print the same way: `[status] ...`,
//! `[alice hint] ...`, `[team Red wins]`, `[audio] asset 2`, and so on.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;

use brdb::schema::BrdbValue;

use crate::collections::HashMap;
use crate::graph::{Gate, GateId};
use crate::value::Value;
use crate::world::{ChatLine, GateInputs, World, WorldEvent, WorldResult};

/// One scripted action.
#[derive(Clone, Debug)]
enum Action {
    Join(String),
    Leave(String),
    Chat {
        player: String,
        command: String,
        arguments: String,
    },
    Round,
    RoundEnd,
    Set(Change),
    Spawn(String),
    Die {
        player: String,
        killer: Option<String>,
    },
    Damage {
        player: String,
        amount: f64,
        attacker: Option<String>,
    },
    Fire {
        player: String,
        weapon: String,
    },
    JoinTeam {
        player: String,
        team: String,
    },
    LeaveTeam(String),
    Interact {
        player: String,
        held: bool,
    },
    Target {
        player: String,
        damage: f64,
        weapon: String,
    },
    Answer {
        player: String,
        answer: Answer,
    },
}

/// A change to the world's state that raises no event: applied before the
/// run at tick 0, or on its tick.
#[derive(Clone, Debug)]
enum Change {
    Move {
        player: String,
        location: Value,
    },
    Input {
        player: String,
        port: &'static str,
        value: Value,
    },
    Aim {
        player: String,
        direction: Value,
    },
    Slot {
        player: String,
        slot: i64,
    },
    Zone(Vec<String>),
}

/// How a player answers a box.
#[derive(Clone, Debug)]
enum Answer {
    Accept,
    Decline,
    Text(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoxKind {
    Message,
    OkCancel,
    YesNo,
    TextInput,
}

/// A box a program showed a player that they have not answered.
#[derive(Clone, Debug)]
struct OpenBox {
    gate: GateId,
    class: String,
    kind: BoxKind,
}

/// The inputs a player holds, as `GetInputs` names its outputs: the axes,
/// then the buttons.
const INPUT_AXES: [&str; 7] = [
    "Forward",
    "Right",
    "Up",
    "Pitch",
    "Yaw",
    "Roll",
    "MouseWheel",
];
const INPUT_BUTTONS: [&str; 5] = ["C", "E", "Q", "LeftMouse", "RightMouse"];

/// The `GetInputs` output an `input` line names, and the value it holds.
fn input_port(name: &str, value: &str) -> Option<(&'static str, Value)> {
    if let Some(i) = INPUT_AXES.iter().position(|a| a.eq_ignore_ascii_case(name)) {
        let v: f64 = value.parse().ok().filter(|f: &f64| f.is_finite())?;
        return Some((
            [
                "InputForward",
                "InputRight",
                "InputUp",
                "InputPitch",
                "InputYaw",
                "InputRoll",
                "InputMouseWheel",
            ][i],
            Value::Float(v),
        ));
    }
    let i = INPUT_BUTTONS
        .iter()
        .position(|b| b.eq_ignore_ascii_case(name))?;
    Some((
        [
            "bPressedC",
            "bPressedE",
            "bPressedQ",
            "bPressedLeftMouse",
            "bPressedRightMouse",
        ][i],
        Value::Bool(parse_bool(value)?),
    ))
}

fn parse_bool(word: &str) -> Option<bool> {
    match word {
        "1" | "true" | "yes" | "on" | "down" | "held" => Some(true),
        "0" | "false" | "no" | "off" | "up" => Some(false),
        _ => None,
    }
}

/// The display name and user name of a player the script does not name.
pub const DEFAULT_NAME: &str = "Wirescript";

/// How much damage a character takes when the script gives no limit.
pub const DEFAULT_DAMAGE_LIMIT: f64 = 100.0;

/// A player: their controller and character objects and who they are.
#[derive(Clone, Debug)]
pub struct Player {
    pub controller: u64,
    pub character: u64,
    pub display_name: String,
    pub user_name: String,
    pub user_id: String,
    /// The texts `DisplayText` shows them, by text id.
    pub screen: BTreeMap<i64, String>,
    /// The inputs they hold, by `GetInputs` output; one never set is `0`
    /// or not pressed.
    pub inputs: BTreeMap<&'static str, Value>,
    next_text_id: i64,
    pub joined: bool,
    /// Their team, an index into [`ScriptWorld::teams`].
    pub team: Option<usize>,
    pub pinned: bool,
    pub permissions: BTreeSet<String>,
    pub roles: BTreeSet<String>,
    pub trusted: bool,
    pub can_respawn: bool,
    pub damage_limit: f64,
    /// The direction they aim; forward (+X) until the script says.
    pub aim: Value,
    /// Their current character's state.
    pub body: Body,
    /// Whether their current character has spawned or died, so the next
    /// spawn makes a new one.
    spawned: bool,
}

/// What a player's character carries. A new character starts empty, at
/// normal size.
#[derive(Clone, Debug)]
pub struct Body {
    pub damage: f64,
    /// Player scale; 1.0 is normal size.
    pub scale: f64,
    /// Ammo by resource.
    pub ammo: BTreeMap<String, i64>,
    /// Chambered ammo by resource and slot.
    pub chamber: BTreeMap<(String, i64), i64>,
    /// What each inventory slot holds.
    pub inventory: BTreeMap<i64, Held>,
    pub slot: i64,
}

impl Default for Body {
    fn default() -> Self {
        Self {
            damage: 0.0,
            scale: 1.0,
            ammo: BTreeMap::new(),
            chamber: BTreeMap::new(),
            inventory: BTreeMap::new(),
            slot: 0,
        }
    }
}

/// What an inventory slot holds, by its asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Held {
    Item(String),
    Brick(String),
    Entity(String),
}

/// A team the script declares.
#[derive(Clone, Debug)]
pub struct Team {
    pub name: String,
    pub id: u64,
}

/// What a sweep reports.
#[derive(Clone, Debug)]
pub struct SweepHit {
    pub brick: Option<Value>,
    pub distance: f64,
    pub location: Value,
}

/// A world a test scripts. It records chat, keeps every setting and property
/// a program writes and reads them back, hands out component handles, and
/// answers sweeps and transforms with what the script set. It models no
/// physics: a setting nothing wrote reads as the script's default for it, or,
/// when the script gave none, white for a color property and `0` for anything
/// else. Velocities are stored values; nothing moves on its own.
#[derive(Default)]
pub struct ScriptWorld {
    pub lines: Vec<ChatLine>,
    /// Settings and properties by key (`env:Sky.TimeOfDay`,
    /// `brick:<brick>:<property>`, `component:<id>:<property>`).
    pub values: HashMap<String, Value>,
    /// What an unwritten setting reads as, by the same key.
    pub defaults: HashMap<String, Value>,
    components: HashMap<(String, String), u64>,
    next_object: u64,
    /// Where the program's microchip is.
    pub own_location: Option<Value>,
    /// The next sweeps' results, in order (`None` misses); a sweep with
    /// none left misses.
    pub sweeps: VecDeque<Option<SweepHit>>,
    /// Print each chat line to stdout as it is said, prefixed with its tick
    /// when `echo_ticks` is set.
    pub echo: bool,
    pub echo_ticks: bool,
    actions: BTreeMap<u64, Vec<Action>>,
    /// Every player the script names, by script name, joined or not.
    pub players: BTreeMap<String, Player>,
    /// Entity locations and rotations a program or the script set, by object
    /// id; an entity nothing set is at the origin, unrotated.
    pub locations: HashMap<u64, Value>,
    pub rotations: HashMap<u64, Value>,
    rounds: i64,
    /// The teams, in the order the script declares them.
    pub teams: Vec<Team>,
    /// Joined players' script names, in the order they joined.
    join_order: Vec<String>,
    /// Entity velocities by object id; one nothing set is still.
    pub linear: HashMap<u64, Value>,
    pub angular: HashMap<u64, Value>,
    pub frozen: HashMap<u64, bool>,
    pub tags: HashMap<u64, String>,
    /// The team of an entity that is not a player's.
    entity_teams: HashMap<u64, u64>,
    /// Entity custom properties by object id, then key.
    pub properties: HashMap<u64, BTreeMap<String, Value>>,
    /// Leaderboard values by player controller or team, then key.
    pub leaderboard: HashMap<(u64, String), i64>,
    /// The players standing in the zone; every zone holds the same players.
    pub zone: Vec<String>,
    /// Where a teleport destination is.
    pub teleport: Option<Value>,
    boxes: BTreeMap<String, VecDeque<OpenBox>>,
    /// Events a gate set off, raised on the next tick.
    queued: Vec<WorldEvent>,
    /// An object for each asset an inventory slot holds.
    assets: HashMap<String, u64>,
    /// Script actions that could not happen.
    problems: Vec<String>,
}

/// `text` with control characters (an escape sequence, a tab, a newline)
/// written as `\u{..}`, so printing it cannot drive the terminal.
pub fn escape_controls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        // Controls, and the invisible format characters that reorder or hide
        // text (bidi overrides, zero-width spaces, line separators).
        let hidden = matches!(
            c,
            '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{2069}'
                | '\u{FEFF}'
        );
        if c.is_control() || hidden {
            let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    out
}

/// `line` without a `#` comment: one that starts the line or follows a
/// space, so `/say #1` keeps its `#1`.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            return &line[..i];
        }
    }
    line
}

/// What follows the first `n` words of `line`, its own spacing kept.
fn after_words(line: &str, n: usize) -> &str {
    let mut rest = line.trim_start();
    for _ in 0..n {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        rest = rest[end..].trim_start();
    }
    rest
}

/// The user id a player the script gives none gets: a version-4-shaped
/// UUID made from their script name, the same on every run.
pub fn default_user_id(name: &str) -> String {
    let hash = |seed: u64| {
        let mut h = 0xcbf2_9ce4_8422_2325_u64 ^ seed;
        for b in name.bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        h
    };
    let (hi, lo) = (hash(0), hash(0x9e37_79b9_7f4a_7c15));
    let hi = (hi & !0xf000) | 0x4000;
    let lo = (lo & !(0xc << 60)) | (0x8 << 60);
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        hi >> 32,
        (hi >> 16) & 0xffff,
        hi & 0xffff,
        lo >> 48,
        lo & 0xffff_ffff_ffff
    )
}

/// Three numbers as a vector.
fn parse_vector(words: &[&str]) -> Option<Value> {
    let n: Vec<f64> = words
        .iter()
        .map(|w| w.parse::<f64>().ok().filter(|f| f.is_finite()))
        .collect::<Option<_>>()?;
    match n.as_slice() {
        [x, y, z] => Some(vector([*x, *y, *z])),
        _ => None,
    }
}

fn parse_number(word: &str) -> Option<f64> {
    word.parse().ok().filter(|f: &f64| f.is_finite())
}

fn origin() -> Value {
    vector([0.0; 3])
}

fn forward() -> Value {
    vector([1.0, 0.0, 0.0])
}

fn unrotated() -> Value {
    Value::Rotator {
        pitch: 0.0,
        yaw: 0.0,
        roll: 0.0,
    }
}

fn vector([x, y, z]: [f64; 3]) -> Value {
    Value::Vector { x, y, z }
}

/// The components of a vector; anything else is the zero vector.
fn xyz(v: &Value) -> [f64; 3] {
    match v {
        Value::Vector { x, y, z } => [*x, *y, *z],
        _ => [0.0; 3],
    }
}

fn add([a, b, c]: [f64; 3], [x, y, z]: [f64; 3]) -> [f64; 3] {
    [a + x, b + y, c + z]
}

fn sub([a, b, c]: [f64; 3], [x, y, z]: [f64; 3]) -> [f64; 3] {
    [a - x, b - y, c - z]
}

/// An angular velocity (degrees a second about X, Y, Z) as a rotator: roll
/// about X, pitch about Y, yaw about Z.
fn spin_rotator([x, y, z]: [f64; 3]) -> Value {
    Value::Rotator {
        pitch: y,
        yaw: z,
        roll: x,
    }
}

fn number(v: f64) -> String {
    Value::Float(v).render()
}

fn event(class: &str, outputs: Vec<(&str, Value)>) -> WorldEvent {
    WorldEvent {
        class: format!("BrickComponentType_{class}"),
        outputs: outputs
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
        ..WorldEvent::default()
    }
}

impl ScriptWorld {
    /// A world running `script`; an error names the line that is wrong.
    pub fn from_script(script: &str) -> Result<Self, String> {
        let mut world = Self::default();
        let mut team_names: Vec<(u64, String)> = Vec::new();
        for (n, line) in script.lines().enumerate() {
            world.add_line(n + 1, line, &mut team_names)?;
        }
        if let Some((tick, team)) = team_names
            .iter()
            .find(|(_, team)| world.team_named(team).is_none())
        {
            return Err(format!(
                "script: the jointeam at tick {tick} names team {team:?}, which no team line declares"
            ));
        }
        Ok(world)
    }

    /// Add script line `n` (`<tick> <action>`). A `jointeam` records its
    /// team in `team_names`, checked once every line is in.
    fn add_line(
        &mut self,
        n: usize,
        line: &str,
        team_names: &mut Vec<(u64, String)>,
    ) -> Result<(), String> {
        let line = strip_comment(line).trim();
        if line.is_empty() {
            return Ok(());
        }
        let bad = |why: &str| format!("script line {n}: {why}: {line:?}");
        let mut words = line.split_whitespace();
        let tick: u64 = words
            .next()
            .and_then(|t| t.parse().ok())
            .ok_or_else(|| bad("expected a tick number first"))?;
        let verb = words.next().ok_or_else(|| bad("expected an action"))?;
        let rest: Vec<&str> = words.collect();
        let setup = |what: &str| {
            if tick == 0 {
                Ok(())
            } else {
                Err(bad(&format!(
                    "a {what} line sets the self up before the run; give it tick 0"
                )))
            }
        };
        let name = |w: &&str| (*w).to_string();
        let change = match (verb, rest.as_slice()) {
            ("move", [player, at @ ..]) => Change::Move {
                player: name(player),
                location: parse_vector(at).ok_or_else(|| bad("expected x y z"))?,
            },
            ("input", [player, port, value]) => {
                let (port, value) = input_port(port, value).ok_or_else(|| {
                    bad("expected an axis (Forward, Right, Up, Pitch, Yaw, Roll, MouseWheel) and a number, or a button (C, E, Q, LeftMouse, RightMouse) and 1 or 0")
                })?;
                Change::Input {
                    player: name(player),
                    port,
                    value,
                }
            }
            ("aim", [player, at @ ..]) => Change::Aim {
                player: name(player),
                direction: parse_vector(at).ok_or_else(|| bad("expected x y z"))?,
            },
            ("slot", [player, slot]) => Change::Slot {
                player: name(player),
                slot: slot.parse().map_err(|_| bad("expected a slot number"))?,
            },
            ("zone", members) => Change::Zone(members.iter().map(name).collect()),
            _ => {
                let action = match (verb, rest.as_slice()) {
                    ("join", [player]) => Action::Join(name(player)),
                    ("leave", [player]) => Action::Leave(name(player)),
                    ("chat", [player, command, ..]) => Action::Chat {
                        player: name(player),
                        command: command
                            .strip_prefix('/')
                            .ok_or_else(|| bad("a chat command starts with /"))?
                            .to_string(),
                        arguments: after_words(line, 4).to_string(),
                    },
                    ("round", []) => Action::Round,
                    ("roundend", []) => Action::RoundEnd,
                    ("sweep", [distance]) => {
                        let distance = parse_number(distance)
                            .filter(|d| *d >= 0.0)
                            .ok_or_else(|| bad("expected a distance of 0 or more"))?;
                        self.sweeps.push_back(Some(SweepHit {
                            brick: None,
                            distance,
                            location: vector([0.0, 0.0, -distance]),
                        }));
                        return Ok(());
                    }
                    ("miss", []) => {
                        self.sweeps.push_back(None);
                        return Ok(());
                    }
                    ("player", [player, field, _, ..]) => {
                        setup("player")?;
                        let value = after_words(line, 4).to_string();
                        let one_word = !value.contains(char::is_whitespace);
                        let p = self.player(player);
                        match *field {
                            "name" => p.display_name = value,
                            "user" if one_word => p.user_name = value,
                            "id" if one_word => p.user_id = value,
                            "user" | "id" => return Err(bad("a user name or id is one word")),
                            "permission" => {
                                p.permissions.insert(value);
                            }
                            "role" => {
                                p.roles.insert(value);
                            }
                            "trusted" => {
                                p.trusted = parse_bool(&value)
                                    .ok_or_else(|| bad("expected yes or no"))?;
                            }
                            "damagelimit" => {
                                p.damage_limit = parse_number(&value)
                                    .filter(|d| *d > 0.0)
                                    .ok_or_else(|| bad("expected a damage limit above 0"))?;
                            }
                            _ => {
                                return Err(bad(
                                    "expected name, user, id, permission, role, trusted or damagelimit",
                                ));
                            }
                        }
                        return Ok(());
                    }
                    ("team", [team, members @ ..]) => {
                        setup("team")?;
                        if self.team_named(team).is_some() {
                            return Err(bad("that team is already declared"));
                        }
                        let id = self.object();
                        self.teams.push(Team {
                            name: name(team),
                            id,
                        });
                        let t = self.teams.len() - 1;
                        for m in members {
                            self.player(m).team = Some(t);
                        }
                        return Ok(());
                    }
                    ("teleport", at) => {
                        setup("teleport")?;
                        self.teleport =
                            Some(parse_vector(at).ok_or_else(|| bad("expected x y z"))?);
                        return Ok(());
                    }
                    ("spawn", [player]) => Action::Spawn(name(player)),
                    ("die", [player, killer @ ..]) if killer.len() <= 1 => Action::Die {
                        player: name(player),
                        killer: killer.first().map(name),
                    },
                    ("damage", [player, amount, attacker @ ..]) if attacker.len() <= 1 => {
                        Action::Damage {
                            player: name(player),
                            amount: parse_number(amount)
                                .ok_or_else(|| bad("expected an amount of damage"))?,
                            attacker: attacker.first().map(name),
                        }
                    }
                    ("fire", [player, ..]) => Action::Fire {
                        player: name(player),
                        weapon: after_words(line, 3).to_string(),
                    },
                    ("jointeam", [player, team]) => {
                        team_names.push((tick, name(team)));
                        Action::JoinTeam {
                            player: name(player),
                            team: name(team),
                        }
                    }
                    ("leaveteam", [player]) => Action::LeaveTeam(name(player)),
                    ("interact", [player, held @ ..]) if held.len() <= 1 => Action::Interact {
                        player: name(player),
                        held: held.first().is_some_and(|w| *w == "held"),
                    },
                    ("target", [player, damage, ..]) => Action::Target {
                        player: name(player),
                        damage: parse_number(damage)
                            .ok_or_else(|| bad("expected an amount of damage"))?,
                        weapon: after_words(line, 4).to_string(),
                    },
                    ("answer", [player, how, ..]) => {
                        let answer = match *how {
                            "yes" | "ok" if rest.len() == 2 => Answer::Accept,
                            "no" | "cancel" if rest.len() == 2 => Answer::Decline,
                            "text" => Answer::Text(after_words(line, 4).to_string()),
                            _ => {
                                return Err(bad(
                                    "expected yes, ok, no, cancel or text <words>",
                                ));
                            }
                        };
                        Action::Answer {
                            player: name(player),
                            answer,
                        }
                    }
                    _ => return Err(bad("unknown action")),
                };
                if tick == 0 {
                    return Err(bad(
                        "tick 0 is the Load Event; self events start at tick 1",
                    ));
                }
                self.actions.entry(tick).or_default().push(action);
                return Ok(());
            }
        };
        if tick == 0 {
            self.apply(change);
        } else {
            self
                .actions
                .entry(tick)
                .or_default()
                .push(Action::Set(change));
        }

        Ok(())
    }

    /// Queue `line`, a script line without its tick, to happen on `tick`.
    pub fn push_line(&mut self, tick: u64, line: &str) -> Result<(), String> {
        let mut team_names = Vec::new();
        self.add_line(1, &format!("{tick} {line}"), &mut team_names)
            .map_err(|e| e.replacen("script line 1: ", "", 1))?;
        match team_names.iter().find(|(_, team)| self.team_named(team).is_none()) {
            Some((_, team)) => Err(format!("no team is named {team:?}")),
            None => Ok(()),
        }
    }

    /// Player `name`, set up with the defaults if they are new.
    fn player(&mut self, name: &str) -> &mut Player {
        if !self.players.contains_key(name) {
            let controller = self.object();
            let character = self.object();
            self.players.insert(
                name.to_string(),
                Player {
                    controller,
                    character,
                    display_name: DEFAULT_NAME.to_string(),
                    user_name: DEFAULT_NAME.to_string(),
                    user_id: default_user_id(name),
                    screen: BTreeMap::new(),
                    inputs: BTreeMap::new(),
                    next_text_id: 1,
                    joined: false,
                    team: None,
                    pinned: false,
                    permissions: BTreeSet::new(),
                    roles: BTreeSet::new(),
                    trusted: false,
                    can_respawn: true,
                    damage_limit: DEFAULT_DAMAGE_LIMIT,
                    aim: forward(),
                    body: Body::default(),
                    spawned: false,
                },
            );
        }
        self.players.get_mut(name).expect("inserted above")
    }

    /// The script name of the player whose controller or character `v` is.
    fn player_of(&self, v: Option<&Value>) -> Option<String> {
        let Some(Value::Object(id)) = v else {
            return None;
        };
        self.players
            .iter()
            .find(|(_, p)| p.controller == *id || p.character == *id)
            .map(|(name, _)| name.clone())
    }

    /// The player on input `port`.
    fn who(&self, inputs: &mut dyn GateInputs, port: &str) -> Result<Option<String>, String> {
        Ok(self.player_of(read(inputs, port)?.as_ref()))
    }

    /// What `who` holds on each of `gate`'s value outputs (`GetInputs` and
    /// the seat control splitter share the names); nobody, or an input never
    /// set, is `0` or not pressed.
    fn held_inputs(&self, who: Option<String>, gate: &Gate) -> Vec<(String, Value)> {
        let held = who.map(|n| self.players[&n].inputs.clone()).unwrap_or_default();
        gate.outputs
            .iter()
            .filter(|o| !o.exec)
            .map(|o| {
                let v = held.get(o.name.as_str()).cloned().unwrap_or_else(|| {
                    if o.name.starts_with("bPressed") {
                        Value::Bool(false)
                    } else {
                        Value::Float(0.0)
                    }
                });
                (o.name.clone(), v)
            })
            .collect()
    }

    fn team_named(&self, name: &str) -> Option<usize> {
        self.teams.iter().position(|t| t.name == name)
    }

    fn team_of_object(&self, v: Option<&Value>) -> Option<usize> {
        let id = object_id(v);
        self.teams.iter().position(|t| id != 0 && t.id == id)
    }

    fn read(&self, key: &str) -> Value {
        self.values
            .get(key)
            .or_else(|| self.defaults.get(key))
            .cloned()
            .unwrap_or_else(|| {
                if key.to_ascii_lowercase().contains("color") {
                    Value::Color {
                        r: 1.0,
                        g: 1.0,
                        b: 1.0,
                        a: 1.0,
                    }
                } else {
                    Value::Float(0.0)
                }
            })
    }

    fn object(&mut self) -> u64 {
        self.next_object += 1;
        self.next_object
    }

    fn say(&mut self, tick: u64, text: &str) {
        World::chat(self, tick, text);
    }

    /// Who object `v` is, for a printed line: a player's script name, a
    /// team, or an entity number.
    fn describe(&self, v: Option<&Value>) -> String {
        match v {
            None | Some(Value::Object(0)) => "nobody".to_string(),
            Some(Value::Object(id)) => {
                if let Some(name) = self.player_of(v) {
                    return name;
                }
                match self.teams.iter().find(|t| t.id == *id) {
                    Some(t) => format!("team {}", t.name),
                    None => format!("entity {id}"),
                }
            }
            Some(v) => v.to_string(),
        }
    }

    fn location_of(&self, id: u64) -> Value {
        self.locations.get(&id).cloned().unwrap_or_else(origin)
    }

    /// A teleport destination or source: a location, an entity's location,
    /// or the script's `teleport` location.
    fn place_of(&self, v: Option<&Value>) -> Result<[f64; 3], String> {
        match v {
            Some(at @ Value::Vector { .. }) => Ok(xyz(at)),
            Some(Value::Object(id)) if *id != 0 => Ok(xyz(&self.location_of(*id))),
            _ => self.teleport.as_ref().map(xyz).ok_or_else(|| {
                "the teleport destination is not a location or an entity; give the script a `0 teleport x y z` line".to_string()
            }),
        }
    }

    fn apply(&mut self, change: Change) {
        match change {
            Change::Move { player, location } => {
                let character = self.player(&player).character;
                self.locations.insert(character, location);
            }
            Change::Input {
                player,
                port,
                value,
            } => {
                self.player(&player).inputs.insert(port, value);
            }
            Change::Aim { player, direction } => self.player(&player).aim = direction,
            Change::Slot { player, slot } => self.player(&player).body.slot = slot,
            Change::Zone(members) => {
                for m in &members {
                    self.player(m);
                }
                self.zone = members;
            }
        }
    }

    /// Give player `name` a new, empty character.
    fn new_character(&mut self, name: &str) {
        let character = self.object();
        let p = self.player(name);
        p.character = character;
        p.body = Body::default();
    }

    /// Move player `name` to team `team` (`None`: no team), with the events a
    /// joined player's move raises.
    fn change_team(&mut self, name: &str, team: Option<usize>) -> Vec<WorldEvent> {
        let p = self.player(name);
        let old = p.team;
        if old == team {
            return Vec::new();
        }
        p.team = team;
        if !p.joined {
            return Vec::new();
        }
        let (controller, user_id, user_name) =
            (p.controller, p.user_id.clone(), p.user_name.clone());
        let mut out = Vec::new();
        for (class, t) in [
            ("Event_Gamemode_ControllerLeftTeam", old),
            ("Event_Gamemode_ControllerJoinedTeam", team),
        ] {
            let Some(t) = t else { continue };
            out.push(event(
                class,
                vec![
                    ("PlayerState", Value::Object(controller)),
                    ("Team", Value::Object(self.teams[t].id)),
                    ("UserId", Value::Str(user_id.clone())),
                    ("UserName", Value::Str(user_name.clone())),
                ],
            ));
        }
        out
    }

    /// The controllers of the joined players `keep` accepts, in join order.
    fn joined_controllers(&self, keep: impl Fn(&Player) -> bool) -> Vec<Value> {
        self.join_order
            .iter()
            .filter_map(|n| self.players.get(n))
            .filter(|p| keep(p))
            .map(|p| Value::Object(p.controller))
            .collect()
    }

    /// The object standing for asset `text`.
    fn asset_object(&mut self, text: &str) -> u64 {
        if let Some(&id) = self.assets.get(text) {
            return id;
        }
        let id = self.object();
        self.assets.insert(text.to_string(), id);
        id
    }

    fn character_of(&self, name: Option<&String>) -> u64 {
        name.and_then(|n| self.players.get(n))
            .map_or(0, |p| p.character)
    }

    /// The events script action `action` raises on `tick`.
    fn run_action(&mut self, tick: u64, action: Action) -> Vec<WorldEvent> {
        let one = |e: WorldEvent| vec![e];
        match action {
            Action::Join(name) => {
                let p = self.player(&name);
                p.joined = true;
                let e = event(
                    "Event_Gamemode_ControllerJoined",
                    vec![
                        ("PlayerState", Value::Object(p.controller)),
                        ("UserId", Value::Str(p.user_id.clone())),
                        ("UserName", Value::Str(p.user_name.clone())),
                    ],
                );
                if !self.join_order.contains(&name) {
                    self.join_order.push(name);
                }
                one(e)
            }
            Action::Leave(name) => {
                let (user_id, user_name, team) = {
                    let p = self.player(&name);
                    (p.user_id.clone(), p.user_name.clone(), p.team)
                };
                let mut out = vec![event(
                    "Event_Gamemode_ControllerLeft",
                    vec![
                        ("UserId", Value::Str(user_id.clone())),
                        ("UserName", Value::Str(user_name.clone())),
                    ],
                )];
                if let Some(t) = team {
                    out.push(event(
                        "Event_Gamemode_ControllerLeftTeam",
                        vec![
                            ("PlayerState", Value::Object(0)),
                            ("Team", Value::Object(self.teams[t].id)),
                            ("UserId", Value::Str(user_id)),
                            ("UserName", Value::Str(user_name)),
                        ],
                    ));
                }
                // References to a player who left read as nobody; joining
                // again makes new objects, as a reconnect does.
                let controller = self.object();
                self.new_character(&name);
                let p = self.player(&name);
                p.controller = controller;
                p.screen.clear();
                p.joined = false;
                p.team = None;
                p.spawned = false;
                self.boxes.remove(&name);
                self.join_order.retain(|n| *n != name);
                out
            }
            Action::Set(change) => {
                self.apply(change);
                Vec::new()
            }
            Action::Chat {
                player,
                command,
                arguments,
            } => {
                let id = self.player(&player).controller;
                let mut e = event(
                    "Event_ChatCommand",
                    vec![
                        ("PlayerState", Value::Object(id)),
                        ("Arguments", Value::Str(arguments)),
                    ],
                );
                e.filter = Some(("CommandName".into(), command));
                one(e)
            }
            Action::Round => {
                self.rounds += 1;
                one(event(
                    "Event_Gamemode_RoundStart",
                    vec![("RoundNumber", Value::Int(self.rounds))],
                ))
            }
            Action::RoundEnd => one(event(
                "Event_Gamemode_RoundEnd",
                vec![("RoundNumber", Value::Int(self.rounds))],
            )),
            Action::Spawn(name) => {
                if self.player(&name).spawned {
                    self.new_character(&name);
                }
                let p = self.player(&name);
                p.spawned = true;
                one(event(
                    "Event_Gamemode_CharacterSpawned",
                    vec![("Character", Value::Object(p.character))],
                ))
            }
            Action::Die { player, killer } => {
                let killer = killer.map(|k| self.player(&k).character).unwrap_or(0);
                let p = self.player(&player);
                p.spawned = true;
                one(event(
                    "Event_Gamemode_CharacterDied",
                    vec![
                        ("Character", Value::Object(p.character)),
                        ("Killer", Value::Object(killer)),
                        ("KillerWeapon", Value::Object(0)),
                        ("KillerWeaponName", Value::Str(String::new())),
                    ],
                ))
            }
            Action::Damage {
                player,
                amount,
                attacker,
            } => {
                let attacker = attacker.map(|k| self.player(&k).character).unwrap_or(0);
                let p = self.player(&player);
                p.body.damage = (p.body.damage + amount).clamp(0.0, p.damage_limit);
                one(event(
                    "Event_Gamemode_CharacterDamaged",
                    vec![
                        ("Character", Value::Object(p.character)),
                        ("Damage", Value::Float(amount)),
                        ("Attacker", Value::Object(attacker)),
                        ("AttackerWeapon", Value::Object(0)),
                        ("AttackerWeaponName", Value::Str(String::new())),
                    ],
                ))
            }
            Action::Fire { player, weapon } => {
                let p = self.player(&player);
                let (character, aim) = (p.character, p.aim.clone());
                let start = self.location_of(character);
                one(event(
                    "Event_Gamemode_CharacterFiredWeapon",
                    vec![
                        ("Character", Value::Object(character)),
                        ("Direction", aim),
                        ("Start", start),
                        ("Weapon", Value::Object(0)),
                        ("WeaponName", Value::Str(weapon)),
                    ],
                ))
            }
            Action::JoinTeam { player, team } => {
                let t = self.team_named(&team);
                self.change_team(&player, t)
            }
            Action::LeaveTeam(player) => self.change_team(&player, None),
            Action::Interact { player, held } => {
                let character = self.player(&player).character;
                let mut e = event(
                    "Event_WholeGridInteracted",
                    vec![
                        ("Character", Value::Object(character)),
                        ("bHeld", Value::Bool(held)),
                    ],
                );
                e.fire = Some("Character".into());
                one(e)
            }
            Action::Target {
                player,
                damage,
                weapon,
            } => {
                let character = self.player(&player).character;
                let mut e = event(
                    "Event_WholeGridTargeted",
                    vec![
                        ("CharacterThatJustHit", Value::Object(character)),
                        ("Damage", Value::Float(damage)),
                        ("WeaponThatJustHit", Value::Object(0)),
                        ("WeaponNameThatJustHit", Value::Str(weapon)),
                    ],
                );
                e.fire = Some("Targeted".into());
                one(e)
            }
            Action::Answer { player, answer } => {
                let Some(open) = self.boxes.get_mut(&player).and_then(VecDeque::pop_front) else {
                    self.problems.push(format!(
                        "the script's answer at tick {tick} found no open box for {player}"
                    ));
                    return Vec::new();
                };
                let controller = self.player(&player).controller;
                let accept = if open.kind == BoxKind::YesNo {
                    "YesClicked"
                } else {
                    "OkClicked"
                };
                let mut outputs = vec![("ClickedBy".to_string(), Value::Object(controller))];
                let fire = match answer {
                    Answer::Accept => accept,
                    Answer::Decline => "Declined",
                    Answer::Text(text) => {
                        if open.kind == BoxKind::TextInput {
                            outputs.push(("Text".to_string(), Value::Str(text)));
                        }
                        accept
                    }
                };
                one(WorldEvent {
                    class: open.class,
                    filter: None,
                    outputs,
                    gate: Some(open.gate),
                    fire: Some(fire.to_string()),
                })
            }
        }
    }
}

/// The text a config field holds, whatever its schema type.
fn config_text(inputs: &dyn GateInputs, name: &str) -> String {
    let gate = inputs.gate();
    gate.field_str(name)
        .or_else(|| gate.field(name).map(|v| format!("{v:?}")))
        .unwrap_or_default()
}

fn key_of(v: Option<&Value>) -> String {
    v.map_or_else(|| "none".to_string(), |v| format!("{v:?}"))
}

fn read(inputs: &mut dyn GateInputs, name: &str) -> Result<Option<Value>, String> {
    inputs.input(name).map_err(|e| e.message)
}

fn read_text(inputs: &mut dyn GateInputs, name: &str) -> Result<String, String> {
    Ok(match read(inputs, name)? {
        None => String::new(),
        Some(Value::Str(s)) => s,
        Some(v) => v.render(),
    })
}

fn read_f64(inputs: &mut dyn GateInputs, name: &str, default: f64) -> Result<f64, String> {
    Ok(read(inputs, name)?
        .and_then(|v| v.as_float())
        .unwrap_or(default))
}

fn read_i64(inputs: &mut dyn GateInputs, name: &str) -> Result<i64, String> {
    Ok(read(inputs, name)?.and_then(|v| v.as_int()).unwrap_or(0))
}

fn read_bool(inputs: &mut dyn GateInputs, name: &str) -> Result<bool, String> {
    Ok(read(inputs, name)?.is_some_and(|v| v.truthy()))
}

fn read_xyz(inputs: &mut dyn GateInputs, name: &str) -> Result<[f64; 3], String> {
    Ok(read(inputs, name)?.as_ref().map_or([0.0; 3], xyz))
}

/// The asset on input `port`: what is wired to it, else the asset the gate
/// stores (`asset 3`, the save's asset table index).
fn asset_text(inputs: &mut dyn GateInputs, port: &str) -> Result<String, String> {
    Ok(match read(inputs, port)? {
        None | Some(Value::Object(0)) => asset_field(inputs.gate().field(port)),
        Some(Value::Str(s)) => s,
        Some(v) => v.to_string(),
    })
}

fn asset_field(v: Option<&BrdbValue>) -> String {
    match v {
        Some(BrdbValue::Asset(Some(n))) => format!("asset {n}"),
        _ => "none".to_string(),
    }
}

/// What a `SetInventoryEntry` / `AddInventoryEntry` gives, from its saved
/// entry plan.
fn entry_held(inputs: &dyn GateInputs) -> Held {
    let plan = inputs.gate().field("EntryPlan");
    let part = |name: &str| match plan {
        Some(BrdbValue::Struct(s)) => match s.get(name) {
            Some(BrdbValue::Asset(Some(n))) => Some(format!("asset {n}")),
            _ => None,
        },
        _ => None,
    };
    part("ItemTypeIfItem").map_or_else(
        || part("EntityTypeIfEntity").map_or_else(|| Held::Item("entry".into()), Held::Entity),
        Held::Item,
    )
}

/// The object id an entity input holds; `0` for nothing.
fn object_id(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::Object(id)) => *id,
        _ => 0,
    }
}

impl World for ScriptWorld {
    fn chat(&mut self, tick: u64, text: &str) {
        if self.echo {
            let text = escape_controls(text);
            if self.echo_ticks {
                println!("{tick}: {text}");
            } else {
                println!("{text}");
            }
        }
        self.lines.push(ChatLine {
            tick,
            text: text.to_string(),
        });
    }

    fn chat_lines(&self) -> &[ChatLine] {
        &self.lines
    }

    fn unfinished(&self) -> Option<String> {
        if let Some((tick, _)) = self.actions.first_key_value() {
            return Some(format!("the script's action at tick {tick} never ran"));
        }
        self.problems.first().cloned()
    }

    fn events(&mut self, tick: u64) -> Vec<WorldEvent> {
        let mut out = std::mem::take(&mut self.queued);
        for action in self.actions.remove(&tick).unwrap_or_default() {
            out.extend(self.run_action(tick, action));
        }
        out
    }

    fn push_line(&mut self, tick: u64, line: &str) -> Result<(), String> {
        Self::push_line(self, tick, line)
    }

    fn screen(&self, name: &str) -> Option<Vec<(i64, String)>> {
        let p = self.players.get(name)?;
        Some(p.screen.iter().map(|(&id, text)| (id, text.clone())).collect())
    }

    fn component_outputs(
        &mut self,
        _tick: u64,
        _id: GateId,
        inputs: &mut dyn GateInputs,
    ) -> Result<Vec<(String, Value)>, String> {
        let class = inputs.gate().class.clone();
        let short = class.rsplit_once("Type_").map_or(class.as_str(), |(_, s)| s);
        // The special teams are the ones a `team` line names `Builder` and
        // `Unaffiliated`; no team at all is unaffiliated too.
        let special = |name: &str, team: Option<usize>| team.is_some_and(|t| self.teams[t].name == name);
        match short {
            "Expr_Gamemode_IsBuilderTeam" => {
                let team = self.team_of_object(read(inputs, "Team")?.as_ref());
                Ok(vec![("bResult".into(), Value::Bool(special("Builder", team)))])
            }
            "Expr_Gamemode_IsUnaffiliatedTeam" => {
                let v = read(inputs, "Team")?;
                let team = self.team_of_object(v.as_ref());
                let none = object_id(v.as_ref()) == 0;
                Ok(vec![("bResult".into(), Value::Bool(none || special("Unaffiliated", team)))])
            }
            _ if class.ends_with("Internal_InputSplitter") => {
                let who = self.who(inputs, "Character")?;
                Ok(self.held_inputs(who, inputs.gate()))
            }
            _ => Err(format!("{class} reads the world, which this run does not script")),
        }
    }

    fn exec_gate(
        &mut self,
        tick: u64,
        id: GateId,
        inputs: &mut dyn GateInputs,
    ) -> Result<WorldResult, String> {
        let class = inputs.gate().class.clone();
        let short = class.strip_prefix("BrickComponentType_").unwrap_or(&class);
        let mut out = WorldResult {
            fire: Some("ExecOut".to_string()),
            ..WorldResult::default()
        };
        let setter = short.starts_with("Exec_Set");
        match short {
            "Exec_GetEnvironmentProperty" | "Exec_SetEnvironmentProperty" => {
                let key = format!(
                    "env:{}.{}",
                    config_text(inputs, "Environment"),
                    config_text(inputs, "PropertyName")
                );
                self.property(inputs, key, setter, &mut out)?;
            }
            "Exec_GetBrickProperty" | "Exec_SetBrickProperty" => {
                let brick = read(inputs, "Brick")?;
                let key = format!(
                    "brick:{}:{}",
                    key_of(brick.as_ref()),
                    config_text(inputs, "Property")
                );
                self.property(inputs, key, setter, &mut out)?;
            }
            "Exec_GetComponentProperty" | "Exec_SetComponentProperty" => {
                let component = read(inputs, "Component")?;
                if matches!(component, None | Some(Value::Object(0))) {
                    return Err("the component reference is empty".into());
                }
                let key = format!(
                    "component:{}:{}",
                    key_of(component.as_ref()),
                    config_text(inputs, "PropertyName")
                );
                self.property(inputs, key, setter, &mut out)?;
            }
            "Exec_AddComponent" | "Exec_GetComponent" | "Exec_RemoveComponent" => {
                let brick = key_of(read(inputs, "Brick")?.as_ref());
                let slot = (brick, config_text(inputs, "ComponentType"));
                match short {
                    "Exec_AddComponent" => {
                        let id = match self.components.get(&slot) {
                            Some(&id) => id,
                            None => {
                                let id = self.object();
                                self.components.insert(slot, id);
                                id
                            }
                        };
                        out.outputs.push(("Component".into(), Value::Object(id)));
                    }
                    "Exec_GetComponent" => {
                        let id = self.components.get(&slot).copied().unwrap_or(0);
                        out.outputs.push(("Component".into(), Value::Object(id)));
                    }
                    _ => {
                        self.components.remove(&slot);
                    }
                }
            }
            "Exec_GetOwnTransform" => {
                let location = self.own_location.clone().unwrap_or_else(origin);
                out.outputs.push(("Location".into(), location));
                out.outputs.push(("Rotation".into(), unrotated()));
            }
            "Exec_Sweep" | "Exec_SweepSimple" => {
                let hit = self.sweeps.pop_front().flatten();
                out.outputs.push(("Hit".into(), Value::Bool(hit.is_some())));
                out.outputs
                    .push(("Miss".into(), Value::Bool(hit.is_none())));
                if let Some(h) = hit {
                    out.outputs
                        .push(("HitDistance".into(), Value::Float(h.distance)));
                    out.outputs.push(("HitLocation".into(), h.location));
                    if let Some(b) = h.brick {
                        out.outputs.push(("HitBrick".into(), b));
                    }
                }
            }
            "Exec_PlayerState_GetUserId"
            | "Exec_PlayerState_GetUserName"
            | "Exec_PlayerState_GetDisplayName" => {
                let who = self.who(inputs, "PlayerState")?;
                let p = who.and_then(|n| self.players.get(&n));
                let (port, text) = match short {
                    "Exec_PlayerState_GetUserId" => ("UserId", p.map(|p| p.user_id.clone())),
                    "Exec_PlayerState_GetUserName" => ("UserName", p.map(|p| p.user_name.clone())),
                    _ => ("DisplayName", p.map(|p| p.display_name.clone())),
                };
                out.outputs
                    .push((port.into(), Value::Str(text.unwrap_or_default())));
            }
            "Exec_PlayerState_GetFromEntity" => {
                let who = self.who(inputs, "Entity")?;
                let id = who.map_or(0, |n| self.players[&n].controller);
                out.outputs.push(("PlayerState".into(), Value::Object(id)));
            }
            "Exec_Character_GetFromController" => {
                let who = self.who(inputs, "PlayerState")?;
                let id = self.character_of(who.as_ref());
                out.outputs.push(("Character".into(), Value::Object(id)));
            }
            "Exec_PlayerState_DisplayText" => {
                let who = self.who(inputs, "PlayerState")?;
                let text = read(inputs, "Text")?.map(|v| match v {
                    Value::Str(s) => s,
                    v => v.render(),
                });
                let mut id = read_i64(inputs, "TextId")?;
                if let Some(name) = who {
                    let p = self.players.get_mut(&name).expect("found above");
                    if id == 0 {
                        id = p.next_text_id;
                        p.next_text_id += 1;
                    }
                    let text = text.unwrap_or_default();
                    if p.screen.get(&id) != Some(&text) {
                        p.screen.insert(id, text.clone());
                        self.say(tick, &format!("[{name} text {id}] {text}"));
                    }
                }
                out.outputs.push(("TextIdOut".into(), Value::Int(id)));
            }
            "Exec_PlayerState_ShowChatMessage" => {
                let who = self.who(inputs, "PlayerState")?;
                let message = read_text(inputs, "Message")?;
                if let Some(name) = who {
                    self.say(tick, &format!("[{name} chat] {message}"));
                }
            }
            "Exec_Entity_GetLocation"
            | "Exec_Entity_GetRotation"
            | "Exec_Entity_GetLocationRotation" => {
                let id = object_id(read(inputs, "Entity")?.as_ref());
                if short != "Exec_Entity_GetRotation" {
                    out.outputs.push(("Vector".into(), self.location_of(id)));
                }
                if short != "Exec_Entity_GetLocation" {
                    let rot = self.rotations.get(&id).cloned().unwrap_or_else(unrotated);
                    out.outputs.push(("Rotation".into(), rot));
                }
            }
            "Exec_Entity_SetLocation"
            | "Exec_Entity_SetRotation"
            | "Exec_Entity_SetLocationRotation" => {
                let id = object_id(read(inputs, "Entity")?.as_ref());
                if id != 0 {
                    if short != "Exec_Entity_SetRotation" {
                        let at = read(inputs, "Vector")?.unwrap_or_else(origin);
                        self.locations.insert(id, at);
                    }
                    if short != "Exec_Entity_SetLocation" {
                        let rot = read(inputs, "Rotation")?.unwrap_or_else(unrotated);
                        self.rotations.insert(id, rot);
                    }
                }
            }
            "Exec_GetInputs" => {
                let who = self.who(inputs, "Player")?;
                out.outputs = self.held_inputs(who, inputs.gate());
            }
            s if s.starts_with("Exec_Gamemode_") => {
                self.gamemode_gate(tick, s, inputs, &mut out)?;
            }
            s if s.starts_with("Exec_Character_") => {
                self.character_gate(tick, s, inputs, &mut out)?;
            }
            s if s.starts_with("Exec_Entity_") => self.entity_gate(tick, s, inputs, &mut out)?,
            s if s.starts_with("Exec_PlayerState_") || s == "Exec_FindPlayer" => {
                self.player_gate(tick, id, s, inputs, &mut out)?;
            }
            _ => self.other_gate(tick, short, inputs, &mut out)?,
        }
        Ok(out)
    }
}

/// The error for a world gate `ScriptWorld` does not answer.
fn unmodeled(inputs: &dyn GateInputs) -> String {
    format!(
        "{} is a world gate this script does not model",
        inputs.gate().class
    )
}

impl ScriptWorld {
    /// A property gate: a setter stores its `Value`, a getter reads it back.
    fn property(
        &mut self,
        inputs: &mut dyn GateInputs,
        key: String,
        setter: bool,
        out: &mut WorldResult,
    ) -> Result<(), String> {
        if setter {
            let v = read(inputs, "Value")?;
            self.values.insert(key, v.unwrap_or_default());
        } else {
            out.outputs.push(("Value".into(), self.read(&key)));
        }
        Ok(())
    }

    fn gamemode_gate(
        &mut self,
        tick: u64,
        short: &str,
        inputs: &mut dyn GateInputs,
        out: &mut WorldResult,
    ) -> Result<(), String> {
        let team_id = |w: &Self, t: Option<usize>| Value::Object(t.map_or(0, |t| w.teams[t].id));
        match short {
            "Exec_Gamemode_GetTeam" => {
                let who = self.who(inputs, "Character")?;
                let t = who.and_then(|n| self.players[&n].team);
                out.outputs.push(("Team".into(), team_id(self, t)));
            }
            "Exec_Gamemode_GetTeamByName" => {
                let name = read_text(inputs, "TeamName")?;
                let t = self.team_named(&name);
                out.outputs.push(("Team".into(), team_id(self, t)));
            }
            "Exec_Gamemode_GetTeamName" => {
                let t = self.team_of_object(read(inputs, "Team")?.as_ref());
                let name = t.map(|t| self.teams[t].name.clone()).unwrap_or_default();
                out.outputs.push(("Name".into(), Value::Str(name)));
            }
            "Exec_Gamemode_SetTeam" => {
                let who = self.who(inputs, "PlayerState")?;
                let t = self.team_of_object(read(inputs, "Team")?.as_ref());
                let pin = read_bool(inputs, "bPinPlayerToTeam")?;
                if let Some(name) = who {
                    let events = self.change_team(&name, t);
                    self.queued.extend(events);
                    if pin {
                        self.player(&name).pinned = true;
                    }
                }
            }
            "Exec_Gamemode_SetTeamPinned" => {
                let who = self.who(inputs, "PlayerState")?;
                let pinned = read_bool(inputs, "bPinned")?;
                if let Some(name) = who {
                    self.player(&name).pinned = pinned;
                    self.say(tick, &format!("[{name} team pinned] {pinned}"));
                }
            }
            "Exec_Gamemode_FillArrayFromPlayers" => {
                let items = self.joined_controllers(|_| true);
                out.arrays.push(("ArrayVarRef".into(), items));
            }
            "Exec_Gamemode_FillArrayFromTeamMembers" => {
                let t = self.team_of_object(read(inputs, "Team")?.as_ref());
                let items = match t {
                    Some(t) => self.joined_controllers(|p| p.team == Some(t)),
                    None => Vec::new(),
                };
                out.arrays.push(("ArrayVarRef".into(), items));
            }
            "Exec_Gamemode_GetLeaderboardValue"
            | "Exec_Gamemode_SetLeaderboardValue"
            | "Exec_Gamemode_IncrementLeaderboardValue"
            | "Exec_Gamemode_GetTeamLeaderboardValue"
            | "Exec_Gamemode_SetTeamLeaderboardValue"
            | "Exec_Gamemode_IncrementTeamLeaderboardValue" => {
                let owner = if short.contains("Team") {
                    "Team"
                } else {
                    "PlayerState"
                };
                let mut owner = object_id(read(inputs, owner)?.as_ref());
                if let Some(name) = self.player_of(Some(&Value::Object(owner))) {
                    owner = self.players[&name].controller;
                }
                let key = (owner, read_text(inputs, "Key")?);
                if short.contains("Get") {
                    let v = self.leaderboard.get(&key).copied().unwrap_or(0);
                    out.outputs.push(("Value".into(), Value::Int(v)));
                } else if owner != 0 {
                    let v = read_i64(inputs, "Value")?;
                    let slot = self.leaderboard.entry(key).or_insert(0);
                    *slot = if short.contains("Increment") {
                        slot.wrapping_add(v)
                    } else {
                        v
                    };
                }
            }
            "Exec_Gamemode_GetCurrentRound" => {
                out.outputs
                    .push(("RoundNumber".into(), Value::Int(self.rounds)));
            }
            "Exec_Gamemode_PlayerWins" => {
                let who = self.who(inputs, "Player")?;
                let team_instead = read_bool(inputs, "bTeamWinsInstead")?;
                if let Some(name) = who {
                    let team = self.players[&name].team.filter(|_| team_instead);
                    let line = match team {
                        Some(t) => format!("[team {} wins]", self.teams[t].name),
                        None => format!("[{name} wins]"),
                    };
                    self.say(tick, &line);
                }
            }
            "Exec_Gamemode_TeamWins" => {
                if let Some(t) = self.team_of_object(read(inputs, "Team")?.as_ref()) {
                    let line = format!("[team {} wins]", self.teams[t].name);
                    self.say(tick, &line);
                }
            }
            "Exec_Gamemode_BroadcastStatusMessage" => {
                let message = read_text(inputs, "Message")?;
                self.say(tick, &format!("[status] {message}"));
            }
            _ => return Err(unmodeled(inputs)),
        }
        Ok(())
    }

    fn character_gate(
        &mut self,
        tick: u64,
        short: &str,
        inputs: &mut dyn GateInputs,
        out: &mut WorldResult,
    ) -> Result<(), String> {
        let who = self.who(inputs, "Character")?;
        let empty = Body::default();
        let body = who.as_ref().map_or(&empty, |n| &self.players[n].body);
        let limit = who
            .as_ref()
            .map_or(DEFAULT_DAMAGE_LIMIT, |n| self.players[n].damage_limit);
        match short {
            "Exec_Character_GetDamage" => {
                out.outputs
                    .push(("Damage".into(), Value::Float(body.damage)));
                out.outputs
                    .push(("DamageLimit".into(), Value::Float(limit)));
            }
            "Exec_Character_GetScale" => {
                out.outputs.push(("Scale".into(), Value::Float(body.scale)));
            }
            "Exec_Character_GetAmmo" => {
                let key = asset_text(inputs, "Resource")?;
                let body = who.as_ref().map_or(&empty, |n| &self.players[n].body);
                let n = body.ammo.get(&key).copied().unwrap_or(0);
                out.outputs.push(("Amount".into(), Value::Int(n)));
            }
            "Exec_Character_GetWeaponChamberAmmo" => {
                let key = (asset_text(inputs, "Resource")?, read_i64(inputs, "Slot")?);
                let body = who.as_ref().map_or(&empty, |n| &self.players[n].body);
                let n = body.chamber.get(&key).copied().unwrap_or(0);
                out.outputs.push(("Amount".into(), Value::Int(n)));
            }
            "Exec_Character_GetInventoryEntry" => {
                let slot = read_i64(inputs, "Slot")?;
                let held = who
                    .as_ref()
                    .and_then(|n| self.players[n].body.inventory.get(&slot).cloned());
                let (item, brick, entity) = match held {
                    Some(Held::Item(a)) => (Some(a), None, None),
                    Some(Held::Brick(a)) => (None, Some(a), None),
                    Some(Held::Entity(a)) => (None, None, Some(a)),
                    None => (None, None, None),
                };
                for (port, asset) in [
                    ("Item", item),
                    ("BrickAsset", brick),
                    ("EntityType", entity),
                ] {
                    let id = asset.map_or(0, |a| self.asset_object(&a));
                    out.outputs.push((port.into(), Value::Object(id)));
                }
            }
            "Exec_Character_GetCurrentInventorySlot" => {
                out.outputs.push(("Slot".into(), Value::Int(body.slot)));
            }
            "Exec_Character_GetAim" => {
                let (origin, aim) = match &who {
                    Some(n) => {
                        let p = &self.players[n];
                        (self.location_of(p.character), p.aim.clone())
                    }
                    None => (origin(), forward()),
                };
                out.outputs.push(("Origin".into(), origin));
                out.outputs.push(("Direction".into(), aim));
            }
            _ => {
                let Some(name) = who else {
                    return if Self::character_setter(short) {
                        Ok(())
                    } else {
                        Err(unmodeled(inputs))
                    };
                };
                self.character_change(tick, &name, short, inputs)?;
            }
        }
        Ok(())
    }

    fn character_setter(short: &str) -> bool {
        matches!(
            short,
            "Exec_Character_SetDamage"
                | "Exec_Character_IncDamage"
                | "Exec_Character_SetScale"
                | "Exec_Character_SetAmmo"
                | "Exec_Character_GrantAmmo"
                | "Exec_Character_SetWeaponChamberAmmo"
                | "Exec_Character_IncWeaponChamberAmmo"
                | "Exec_Character_SetInventoryItem"
                | "Exec_Character_SetInventoryItemAdv"
                | "Exec_Character_SetInventoryBrick"
                | "Exec_Character_SetInventoryEntity"
                | "Exec_Character_SetInventoryEntry"
                | "Exec_Character_AddInventoryItem"
                | "Exec_Character_AddInventoryItemAdv"
                | "Exec_Character_AddInventoryBrick"
                | "Exec_Character_AddInventoryEntity"
                | "Exec_Character_AddInventoryEntry"
                | "Exec_Character_ShowHint"
                | "Exec_Character_SetTempPermission"
        )
    }

    /// A character gate that changes player `name`'s character.
    fn character_change(
        &mut self,
        tick: u64,
        name: &str,
        short: &str,
        inputs: &mut dyn GateInputs,
    ) -> Result<(), String> {
        let held = match short {
            "Exec_Character_SetInventoryItem" | "Exec_Character_AddInventoryItem" => {
                Some(Held::Item(asset_text(inputs, "Item")?))
            }
            "Exec_Character_SetInventoryItemAdv" | "Exec_Character_AddInventoryItemAdv" => {
                Some(Held::Item(asset_text(inputs, "ItemType")?))
            }
            "Exec_Character_SetInventoryBrick" | "Exec_Character_AddInventoryBrick" => {
                Some(Held::Brick(asset_text(inputs, "BrickAsset")?))
            }
            "Exec_Character_SetInventoryEntity" | "Exec_Character_AddInventoryEntity" => {
                Some(Held::Entity(asset_text(inputs, "EntityType")?))
            }
            "Exec_Character_SetInventoryEntry" | "Exec_Character_AddInventoryEntry" => {
                Some(entry_held(inputs))
            }
            _ => None,
        };
        if let Some(held) = held {
            let slot = read_i64(inputs, "Slot")?;
            let inventory = &mut self.player(name).body.inventory;
            let slot = if short.contains("_Add") {
                (0..=i64::MAX)
                    .find(|s| !inventory.contains_key(s))
                    .unwrap_or(0)
            } else {
                slot
            };
            inventory.insert(slot, held);
            return Ok(());
        }
        match short {
            "Exec_Character_SetDamage" | "Exec_Character_IncDamage" => {
                let port = if short.ends_with("SetDamage") {
                    "Damage"
                } else {
                    "Amount"
                };
                let v = read_f64(inputs, port, 0.0)?;
                let p = self.player(name);
                let to = if short.ends_with("SetDamage") {
                    v
                } else {
                    p.body.damage + v
                };
                p.body.damage = to.clamp(0.0, p.damage_limit);
            }
            "Exec_Character_SetScale" => {
                self.player(name).body.scale = read_f64(inputs, "Scale", 1.0)?;
            }
            "Exec_Character_SetAmmo" | "Exec_Character_GrantAmmo" => {
                let key = asset_text(inputs, "Resource")?;
                let n = read_i64(inputs, "Amount")?;
                let slot = self.player(name).body.ammo.entry(key).or_insert(0);
                *slot = if short.ends_with("SetAmmo") {
                    n
                } else {
                    slot.saturating_add(n)
                }
                .max(0);
            }
            "Exec_Character_SetWeaponChamberAmmo" | "Exec_Character_IncWeaponChamberAmmo" => {
                let key = (asset_text(inputs, "Resource")?, read_i64(inputs, "Slot")?);
                let n = read_i64(inputs, "Amount")?;
                let slot = self.player(name).body.chamber.entry(key).or_insert(0);
                *slot = if short.contains("_Set") {
                    n
                } else {
                    slot.saturating_add(n)
                }
                .max(0);
            }
            "Exec_Character_ShowHint" => {
                let title = config_text(inputs, "HintTitle");
                let text = config_text(inputs, "HintText");
                let line = if title.is_empty() {
                    format!("[{name} hint] {text}")
                } else {
                    format!("[{name} hint] {title}: {text}")
                };
                self.say(tick, &line);
            }
            "Exec_Character_SetTempPermission" => {
                let permission = config_text(inputs, "PermissionTagStr");
                let on = if read_bool(inputs, "bPermissionEnable")? {
                    "on"
                } else {
                    "off"
                };
                self.say(tick, &format!("[{name} temp permission] {permission} {on}"));
            }
            _ => return Err(unmodeled(inputs)),
        }
        Ok(())
    }

    fn entity_gate(
        &mut self,
        tick: u64,
        short: &str,
        inputs: &mut dyn GateInputs,
        out: &mut WorldResult,
    ) -> Result<(), String> {
        let entity = read(inputs, "Entity")?;
        let id = object_id(entity.as_ref());
        let linear = self.linear.get(&id).map_or([0.0; 3], xyz);
        let angular = self.angular.get(&id).map_or([0.0; 3], xyz);
        let player = self.player_of(entity.as_ref());
        match short {
            "Exec_Entity_GetLinearVelocity" => {
                out.outputs.push(("LinearVelocity".into(), vector(linear)));
            }
            "Exec_Entity_GetAngularVelocity" => {
                out.outputs
                    .push(("AngularVelocity".into(), vector(angular)));
            }
            "Exec_Entity_GetVelocity" => {
                out.outputs.push(("Vector".into(), vector(linear)));
                out.outputs.push(("Rotation".into(), spin_rotator(angular)));
            }
            "Exec_Entity_GetSpeed" => {
                let [x, y, z] = linear;
                out.outputs
                    .push(("Speed".into(), Value::Float((x * x + y * y + z * z).sqrt())));
            }
            "Exec_Entity_GetVelocityAtPoint" => {
                let point = read_xyz(inputs, "Point")?;
                let [rx, ry, rz] = sub(point, xyz(&self.location_of(id)));
                let [wx, wy, wz] = angular.map(f64::to_radians);
                let spin = [wy * rz - wz * ry, wz * rx - wx * rz, wx * ry - wy * rx];
                out.outputs
                    .push(("LinearVelocity".into(), vector(add(linear, spin))));
            }
            "Exec_Entity_IsFrozen" => {
                let frozen = self.frozen.get(&id).copied().unwrap_or(false);
                out.outputs.push(("bFrozen".into(), Value::Bool(frozen)));
            }
            "Exec_Entity_GetTag" => {
                let tag = self.tags.get(&id).cloned().unwrap_or_default();
                out.outputs.push(("Tag".into(), Value::Str(tag)));
            }
            "Exec_Entity_GetTeam" => {
                let team = match &player {
                    Some(n) => self.players[n].team.map_or(0, |t| self.teams[t].id),
                    None => self.entity_teams.get(&id).copied().unwrap_or(0),
                };
                out.outputs.push(("Team".into(), Value::Object(team)));
            }
            "Exec_Entity_GetCustomProperty" | "Exec_Entity_RemoveCustomProperty" => {
                let key = key_of(read(inputs, "Key")?.as_ref());
                let props = self.properties.entry(id).or_default();
                let found = if short.contains("Remove") {
                    props.remove(&key)
                } else {
                    props.get(&key).cloned()
                };
                out.outputs
                    .push(("bFound".into(), Value::Bool(found.is_some())));
                if let Some(v) = found.filter(|_| short.contains("Get")) {
                    out.outputs.push(("Value".into(), v));
                }
            }
            _ if id == 0 => {}
            "Exec_Entity_SetLinearVelocity" => {
                let v = read_xyz(inputs, "LinearVelocity")?;
                self.linear.insert(id, vector(v));
            }
            "Exec_Entity_SetAngularVelocity" => {
                let v = read_xyz(inputs, "AngularVelocity")?;
                self.angular.insert(id, vector(v));
            }
            "Exec_Entity_SetVelocity" | "Exec_Entity_AddVelocity" => {
                let (mut v, mut w) = (read_xyz(inputs, "Vector")?, read_xyz(inputs, "Rotation")?);
                if short.contains("Add") {
                    v = add(v, linear);
                    w = add(w, angular);
                }
                self.linear.insert(id, vector(v));
                self.angular.insert(id, vector(w));
            }
            "Exec_Entity_AddLocationRotation" => {
                let at = add(xyz(&self.location_of(id)), read_xyz(inputs, "Vector")?);
                self.locations.insert(id, vector(at));
                let by = read(inputs, "Rotation")?;
                if let (
                    Some(Value::Rotator { pitch, yaw, roll }),
                    Value::Rotator {
                        pitch: p,
                        yaw: y,
                        roll: r,
                    },
                ) = (
                    by,
                    self.rotations.get(&id).cloned().unwrap_or_else(unrotated),
                ) {
                    self.rotations.insert(
                        id,
                        Value::Rotator {
                            pitch: p + pitch,
                            yaw: y + yaw,
                            roll: r + roll,
                        },
                    );
                }
            }
            "Exec_Entity_Teleport" => {
                let to = self.place_of(read(inputs, "Destination")?.as_ref())?;
                self.locations.insert(id, vector(to));
            }
            "Exec_Entity_RelativeTeleport" => {
                let from = self.place_of(read(inputs, "Source")?.as_ref())?;
                let to = self.place_of(read(inputs, "Destination")?.as_ref())?;
                let at = add(xyz(&self.location_of(id)), sub(to, from));
                self.locations.insert(id, vector(at));
            }
            "Exec_Entity_SetFrozen" => {
                let frozen = read_bool(inputs, "bFrozen")?;
                self.frozen.insert(id, frozen);
            }
            "Exec_Entity_SetTag" => {
                let tag = read_text(inputs, "Tag")?;
                self.tags.insert(id, tag);
            }
            "Exec_Entity_SetTeam" => {
                let team = read(inputs, "Team")?;
                match &player {
                    Some(n) => {
                        let t = self.team_of_object(team.as_ref());
                        let events = self.change_team(n, t);
                        self.queued.extend(events);
                    }
                    None => {
                        self.entity_teams.insert(id, object_id(team.as_ref()));
                    }
                }
            }
            "Exec_Entity_SetCustomProperty" => {
                let key = key_of(read(inputs, "Key")?.as_ref());
                let v = read(inputs, "Value")?.unwrap_or_default();
                self.properties.entry(id).or_default().insert(key, v);
            }
            "Exec_Entity_CopyCustomProperties" => {
                let source = object_id(read(inputs, "Source")?.as_ref());
                let copied = self.properties.get(&source).cloned().unwrap_or_default();
                self.properties.entry(id).or_default().extend(copied);
            }
            "Exec_Entity_SetVisible" => {
                let visible = read_bool(inputs, "bVisible")?;
                let who = self.describe(entity.as_ref());
                self.say(tick, &format!("[{who} visible] {visible}"));
            }
            "Exec_Entity_SetVisibleTo" => {
                let to = read(inputs, "VisibleTo")?;
                let (who, to) = (self.describe(entity.as_ref()), self.describe(to.as_ref()));
                self.say(tick, &format!("[{who} visible to] {to}"));
            }
            "Exec_Entity_SetGravityDirection" => {
                let rot = read(inputs, "Rotation")?.unwrap_or_else(unrotated);
                let who = self.describe(entity.as_ref());
                self.say(tick, &format!("[{who} gravity] {}", rot.render()));
            }
            "Exec_Entity_DestroySpawned" | "Exec_Entity_DestroySpawnedPrefab" => {
                let what = if short.ends_with("Prefab") {
                    "prefab destroyed"
                } else {
                    "destroyed"
                };
                let who = self.describe(entity.as_ref());
                self.say(tick, &format!("[{who} {what}]"));
            }
            _ => return Err(unmodeled(inputs)),
        }
        Ok(())
    }

    fn player_gate(
        &mut self,
        tick: u64,
        id: GateId,
        short: &str,
        inputs: &mut dyn GateInputs,
        out: &mut WorldResult,
    ) -> Result<(), String> {
        if short == "Exec_FindPlayer" {
            let query = read_text(inputs, "Query")?.to_lowercase();
            let names = |p: &Player| [p.display_name.to_lowercase(), p.user_name.to_lowercase()];
            let joined: Vec<&Player> = self
                .join_order
                .iter()
                .filter_map(|n| self.players.get(n))
                .collect();
            let found = joined
                .iter()
                .find(|p| names(p).contains(&query))
                .or_else(|| {
                    joined
                        .iter()
                        .find(|p| !query.is_empty() && names(p).iter().any(|n| n.contains(&query)))
                });
            let id = found.map_or(0, |p| p.controller);
            out.outputs.push(("Player".into(), Value::Object(id)));
            return Ok(());
        }
        let who = self.who(inputs, "PlayerState")?;
        let p = who.as_ref().map(|n| &self.players[n]);
        match short {
            "Exec_PlayerState_HasPermission" => {
                let name = config_text(inputs, "PermissionName");
                let has = p.is_some_and(|p| p.permissions.contains(&name));
                out.outputs
                    .push(("bHasPermission".into(), Value::Bool(has)));
            }
            "Exec_PlayerState_HasRole" => {
                let name = config_text(inputs, "RoleName");
                let has = p.is_some_and(|p| p.roles.contains(&name));
                out.outputs.push(("bHasRole".into(), Value::Bool(has)));
            }
            "Exec_PlayerState_IsTrustedByBrickOwner" => {
                let trusted = p.is_some_and(|p| p.trusted);
                out.outputs
                    .push(("bIsTrusted".into(), Value::Bool(trusted)));
            }
            _ => {
                let Some(name) = who else {
                    return Ok(());
                };
                self.player_change(tick, id, &name, short, inputs)?;
            }
        }
        Ok(())
    }

    /// A player gate that does something to player `name`.
    fn player_change(
        &mut self,
        tick: u64,
        id: GateId,
        name: &str,
        short: &str,
        inputs: &mut dyn GateInputs,
    ) -> Result<(), String> {
        let kind = match short {
            "Exec_PlayerState_ShowMessageBox" => Some((BoxKind::Message, "message box")),
            "Exec_PlayerState_ShowOkCancelBox" => Some((BoxKind::OkCancel, "ok/cancel box")),
            "Exec_PlayerState_ShowYesNoBox" => Some((BoxKind::YesNo, "yes/no box")),
            "Exec_PlayerState_ShowTextInputBox" => Some((BoxKind::TextInput, "text box")),
            _ => None,
        };
        if let Some((kind, label)) = kind {
            let title = read_text(inputs, "Title")?;
            let message = read_text(inputs, "Message")?;
            self.boxes
                .entry(name.to_string())
                .or_default()
                .push_back(OpenBox {
                    gate: id,
                    class: inputs.gate().class.clone(),
                    kind,
                });
            self.say(tick, &format!("[{name} {label}] {title}: {message}"));
            return Ok(());
        }
        match short {
            "Exec_PlayerState_ForceRespawn" => {
                self.new_character(name);
                let p = self.player(name);
                p.spawned = true;
                let character = p.character;
                self.queued.push(event(
                    "Event_Gamemode_CharacterSpawned",
                    vec![("Character", Value::Object(character))],
                ));
                self.say(tick, &format!("[{name} respawn]"));
            }
            "Exec_PlayerState_SetCanRespawn" => {
                let can = read_bool(inputs, "bCanRespawn")?;
                self.player(name).can_respawn = can;
                self.say(tick, &format!("[{name} can respawn] {can}"));
            }
            "Exec_PlayerState_ShowStatusMessage" => {
                let message = read_text(inputs, "Message")?;
                self.say(tick, &format!("[{name} status] {message}"));
            }
            _ => return Err(unmodeled(inputs)),
        }
        Ok(())
    }

    fn other_gate(
        &mut self,
        tick: u64,
        short: &str,
        inputs: &mut dyn GateInputs,
        out: &mut WorldResult,
    ) -> Result<(), String> {
        match short {
            "Exec_Zone_GetEntities" | "Exec_Zone_GetPlayers" => {
                let filter = read_text(inputs, "TagFilter")?;
                let items = self
                    .zone
                    .iter()
                    .filter_map(|n| self.players.get(n))
                    .map(|p| p.character)
                    .filter(|c| filter.is_empty() || self.tags.get(c) == Some(&filter))
                    .map(Value::Object)
                    .collect();
                out.arrays.push(("ArrayVarRef".into(), items));
            }
            "Exec_PlayClientAudio" | "Exec_PlayGlobalAudio" => {
                let asset = audio_text(inputs)?;
                let who = if short == "Exec_PlayClientAudio" {
                    match self.who(inputs, "Player")? {
                        Some(n) => format!("{n} audio"),
                        None => return Ok(()),
                    }
                } else {
                    "audio".to_string()
                };
                self.say(tick, &format!("[{who}] {asset}"));
            }
            "WireGraph_PlayAudioAt" | "Component_WireGraph_PlayAudioAt" => {
                let asset = audio_text(inputs)?;
                let at = self.location_of(object_id(read(inputs, "Entity")?.as_ref()));
                self.say(tick, &format!("[audio at {}] {asset}", at.render()));
            }
            "Exec_SpawnExplosion" | "Exec_SpawnExplosionAt" => {
                let at = if short.ends_with("At") {
                    read_xyz(inputs, "WorldPosition")?
                } else {
                    let own = self.own_location.as_ref().map_or([0.0; 3], xyz);
                    add(own, read_xyz(inputs, "SpawnOffset")?)
                };
                let projectile = asset_text(inputs, "ProjectileType")?;
                self.say(
                    tick,
                    &format!("[explosion at {}] {projectile}", vector(at).render()),
                );
            }
            _ => return Err(unmodeled(inputs)),
        }
        Ok(())
    }
}

/// An audio gate's sound, with its volume and pitch when they are not 1.
fn audio_text(inputs: &mut dyn GateInputs) -> Result<String, String> {
    let mut text = asset_text(inputs, "AudioDescriptor")?;
    for (port, label) in [("VolumeMultiplier", "volume"), ("PitchMultiplier", "pitch")] {
        let v = read_f64(inputs, port, 1.0)?;
        if v != 1.0 {
            let _ = write!(text, " {label} {}", number(v));
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Gate;
    use crate::load::BrickAddr;

    /// A gate of `class` whose inputs read from `values`.
    struct Fake {
        gate: Gate,
        values: Vec<(&'static str, Value)>,
    }

    impl Fake {
        fn new(class: &str, values: Vec<(&'static str, Value)>) -> Self {
            let addr = BrickAddr {
                grid: 1,
                chunk: (0, 0, 0),
                brick: 0,
            };
            Self {
                gate: Gate::new(&format!("BrickComponentType_{class}"), addr),
                values,
            }
        }
    }

    impl GateInputs for Fake {
        fn gate(&self) -> &Gate {
            &self.gate
        }

        fn input(&mut self, name: &str) -> Result<Option<Value>, crate::SimError> {
            Ok(self
                .values
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.clone()))
        }
    }

    fn run(
        world: &mut ScriptWorld,
        class: &str,
        values: Vec<(&'static str, Value)>,
    ) -> WorldResult {
        world
            .exec_gate(1, 7, &mut Fake::new(class, values))
            .expect("the gate runs")
    }

    fn joined(script: &str) -> ScriptWorld {
        let mut world = ScriptWorld::from_script(script).expect("the script parses");
        for tick in 1..=10 {
            world.events(tick);
        }
        world
    }

    #[test]
    fn fills_arrays_with_players_teams_and_zones() {
        let mut w = joined("0 team Red alice\n0 zone alice bob\n2 join bob\n3 join alice\n");
        let (alice, bob) = (&w.players["alice"], &w.players["bob"]);
        let (ac, bc, ach) = (alice.controller, bob.controller, alice.character);
        let red = Value::Object(w.teams[0].id);
        let fill = |r: WorldResult| r.arrays.into_iter().next().expect("an array fill");
        let all = fill(run(&mut w, "Exec_Gamemode_FillArrayFromPlayers", vec![]));
        assert_eq!(
            all,
            (
                "ArrayVarRef".into(),
                vec![Value::Object(bc), Value::Object(ac)]
            )
        );
        let team = fill(run(
            &mut w,
            "Exec_Gamemode_FillArrayFromTeamMembers",
            vec![("Team", red)],
        ));
        assert_eq!(team.1, vec![Value::Object(ac)]);
        w.tags.insert(ach, "runner".into());
        let tagged = fill(run(
            &mut w,
            "Exec_Zone_GetEntities",
            vec![("TagFilter", Value::Str("runner".into()))],
        ));
        assert_eq!(tagged.1, vec![Value::Object(ach)]);
        let zone = fill(run(&mut w, "Exec_Zone_GetPlayers", vec![]));
        assert_eq!(zone.1.len(), 2);
    }

    #[test]
    fn an_answer_fires_the_box_it_answers() {
        let mut w = joined("2 join alice\n");
        let alice = Value::Object(w.players["alice"].controller);
        run(
            &mut w,
            "Exec_PlayerState_ShowTextInputBox",
            vec![("PlayerState", alice.clone())],
        );
        w.actions.entry(20).or_default().push(Action::Answer {
            player: "alice".into(),
            answer: Answer::Text("Ann".into()),
        });
        let events = w.events(20);
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!((e.gate, e.fire.as_deref()), (Some(7), Some("OkClicked")));
        assert!(
            e.outputs
                .contains(&("Text".into(), Value::Str("Ann".into())))
        );
        assert!(e.outputs.contains(&("ClickedBy".into(), alice)));
        w.actions.entry(21).or_default().push(Action::Answer {
            player: "alice".into(),
            answer: Answer::Accept,
        });
        assert!(w.events(21).is_empty());
        assert!(w.unfinished().is_some_and(|p| p.contains("no open box")));
    }

    #[test]
    fn grid_events_fire_their_own_outputs() {
        let mut w =
            ScriptWorld::from_script("3 interact alice held\n4 target alice 5 Bow\n").unwrap();
        let interact = w.events(3);
        assert_eq!(interact[0].fire.as_deref(), Some("Character"));
        assert!(
            interact[0]
                .outputs
                .contains(&("bHeld".into(), Value::Bool(true)))
        );
        assert_eq!(w.events(4)[0].fire.as_deref(), Some("Targeted"));
    }

    #[test]
    fn rejects_undeclared_teams_and_late_setup() {
        assert!(ScriptWorld::from_script("3 jointeam alice Green\n").is_err());
        assert!(ScriptWorld::from_script("3 team Red alice\n").is_err());
        assert!(ScriptWorld::from_script("0 team Red\n0 team Red\n").is_err());
        assert!(ScriptWorld::from_script("3 answer alice maybe\n").is_err());
    }
}
