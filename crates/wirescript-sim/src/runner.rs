//! Compile a Wirescript source and run it, or one of its tests, in the
//! simulator: the one place the CLI, the playground and the editor get a
//! test's verdict from.
//!
//! A test (`@test fn`, `mod` or `chip`) fails when it prints a chat line
//! starting with the word `FAIL`, the simulation stops with an error, it never
//! prints the [`wirescript::TEST_DONE`] line the test entry adds after it, or
//! work it started (an event in flight, a scripted action) is still waiting
//! when the run ends.

use wirescript::emit::EmitOptions;
use wirescript::resolve::FileLoader;

use crate::script_world::escape_controls;
use crate::{Dt, Loaded, ScriptWorld, SimOptions};

/// How a run goes.
#[derive(Clone, Debug)]
pub struct RunSettings {
    pub ticks: u64,
    pub dt: f64,
    pub seed: Option<u64>,
}

/// What one run printed, its sentinel line removed, and the error that
/// stopped it, if any.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub lines: Vec<(u64, String)>,
    pub error: Option<String>,
}

impl Outcome {
    fn failed(error: String) -> Self {
        Self {
            lines: Vec::new(),
            error: Some(error),
        }
    }

    /// Why a test run failed: its `FAIL` lines and its error.
    pub fn problems(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .lines
            .iter()
            .filter(|(_, l)| is_failure(l))
            .map(|(_, l)| l.clone())
            .collect();
        if let Some(e) = &self.error {
            out.push(format!("error: {e}"));
        }
        out
    }
}

/// A chat line that reports a failure: a line of it starts with the word
/// `FAIL`.
pub fn is_failure(text: &str) -> bool {
    text.split('\n').any(|line| {
        let line = line.trim_start();
        line.strip_prefix("FAIL")
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', ' ', '!', '-']))
    })
}

/// A program compiled for the simulator, ready for any number of runs.
pub struct Prepared {
    loaded: Loaded,
    test: bool,
    script: Option<String>,
}

/// Compile `source` (calling test `entry` once on load, if given) against
/// `loader`. `script` drives the world of every run.
pub fn prepare(
    source: &str,
    file: &str,
    entry: Option<&str>,
    script: Option<&str>,
    emit: &EmitOptions,
    loader: &(dyn FileLoader + Sync),
) -> Result<Prepared, String> {
    if let Some(script) = script {
        ScriptWorld::from_script(script)?;
    }
    let mut input = wirescript::CompileInput::new(source, file).test_build(true);
    if let Some(entry) = entry {
        input = input.test_entry(entry);
    }
    let result = wirescript::compile_to_world_with_loader(input, emit.clone(), loader)
        .map_err(|e| format!("compile: {e}"))?;
    let loaded = Loaded::from_world(&result.world, &result.node_bricks, &result.program)
        .map_err(|e| e.to_string())?;
    Ok(Prepared {
        loaded,
        test: entry.is_some(),
        script: script.map(str::to_string),
    })
}

impl Prepared {
    /// A run under `settings`, then one per seed below `race`.
    pub fn run_with_race(&self, settings: &RunSettings, race: u64) -> Vec<Outcome> {
        std::iter::once(settings.seed)
            .chain((0..race).map(Some))
            .map(|seed| {
                self.run(&RunSettings {
                    seed,
                    ..settings.clone()
                })
            })
            .collect()
    }

    /// One run under `settings`, a panic in the simulator reported as its
    /// error.
    pub fn run(&self, settings: &RunSettings) -> Outcome {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_inner(settings)))
            .unwrap_or_else(|payload| {
                let what = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
                    .unwrap_or_default();
                Outcome::failed(format!("the simulator panicked: {what}"))
            })
    }

    fn run_inner(&self, settings: &RunSettings) -> Outcome {
        let world = match self.script.as_deref().map(ScriptWorld::from_script).transpose() {
            Ok(w) => w.unwrap_or_default(),
            Err(e) => return Outcome::failed(e),
        };
        let opts = SimOptions {
            seed: settings.seed,
            dt: Dt::Constant(settings.dt),
        };
        let mut sim = self.loaded.start(Box::new(world), opts);
        let mut error = sim.run_until(settings.ticks).err().map(|e| e.to_string());
        let mut done = false;
        let mut lines = Vec::new();
        for l in sim.chat() {
            if self.test && l.text == wirescript::TEST_DONE {
                done = true;
            } else {
                lines.push((l.tick, l.text.clone()));
            }
        }
        if error.is_none() {
            error = sim.machine.world.unfinished();
        }
        if error.is_none() && self.test {
            if !done {
                error = Some(format!(
                    "the test did not reach its end within {} ticks",
                    settings.ticks
                ));
            } else if let Some(w) = sim.machine.pending_work() {
                error = Some(format!("the run ended at tick {} while {w}", settings.ticks));
            }
        }
        Outcome { lines, error }
    }
}

/// Where a run's chat differs from the text of its `.expected` file, one
/// entry per differing line, plus the run's error. Lines compare as `sim`
/// prints them: control characters escaped, trailing spaces trimmed.
pub fn expected_problems(expected: &str, outcome: &Outcome) -> Vec<String> {
    let want: Vec<&str> = expected.lines().map(str::trim_end).collect();
    let got: Vec<String> = outcome
        .lines
        .iter()
        .map(|(_, l)| escape_controls(l).trim_end().to_string())
        .collect();
    let mut diff = Vec::new();
    for i in 0..want.len().max(got.len()) {
        match (want.get(i), got.get(i)) {
            (Some(w), Some(g)) if *w == g => {}
            (Some(w), Some(g)) => diff.push(format!("line {}: want {w:?}, got {g:?}", i + 1)),
            (Some(w), None) => diff.push(format!("line {}: want {w:?}, got nothing", i + 1)),
            (None, Some(g)) => diff.push(format!("line {}: unexpected {g:?}", i + 1)),
            (None, None) => {}
        }
    }
    if let Some(e) = &outcome.error {
        diff.push(format!("error: {e}"));
    }
    diff
}

/// Where a race run's chat first differs from the first run's, if anywhere.
pub fn race_problem(outcomes: &[Outcome]) -> Option<String> {
    let (base, rest) = outcomes.split_first()?;
    let text = |o: &Outcome, i: usize| o.lines.get(i).map(|(_, t)| t.clone());
    for (seed, o) in rest.iter().enumerate() {
        let n = base.lines.len().max(o.lines.len());
        let show = |v: Option<String>| v.map_or("nothing".to_string(), |s| format!("{s:?}"));
        if let Some(i) = (0..n).find(|&i| text(base, i) != text(o, i)) {
            return Some(format!(
                "race: the result depends on handler order; line {} is {} in save order but {} under seed {seed}",
                i + 1,
                show(text(base, i)),
                show(text(o, i)),
            ));
        }
        if base.error.is_some() != o.error.is_some() {
            return Some(format!(
                "race: the run errors in one handler order and not another (seed {seed})"
            ));
        }
    }
    None
}
