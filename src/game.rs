//! Pure game rules. No Bevy dependency, so the logic is unit-testable and is
//! shared unchanged by the networked server and every client.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

// ---------------------------------------------------------------------------
// Basic types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Terrain {
    Plain,
    Water,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
}

impl Pos {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    pub fn step(self, dir: Dir) -> Pos {
        match dir {
            Dir::Up => Pos::new(self.x, self.y + 1),
            Dir::Down => Pos::new(self.x, self.y - 1),
            Dir::Left => Pos::new(self.x - 1, self.y),
            Dir::Right => Pos::new(self.x + 1, self.y),
        }
    }

    pub fn manhattan(self, other: Pos) -> i32 {
        (self.x - other.x).abs() + (self.y - other.y).abs()
    }

    /// King-move distance: diagonal neighbours are 1 apart.
    pub fn chebyshev(self, other: Pos) -> i32 {
        (self.x - other.x).abs().max((self.y - other.y).abs())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

pub const DIRS: [Dir; 4] = [Dir::Up, Dir::Down, Dir::Left, Dir::Right];

/// Bases must keep this many free tiles between them in every direction,
/// diagonals included.
pub const BASE_GAP: i32 = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum UnitKind {
    /// Fights and captures bases.
    Tank,
    /// Strength 0, never captures, cannot be raised. Founds new bases.
    Settler,
}

impl UnitKind {
    pub fn name(self) -> &'static str {
        match self {
            UnitKind::Tank => "tank",
            UnitKind::Settler => "settler",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub id: u32,
    pub kind: UnitKind,
    pub owner: usize,
    pub pos: Pos,
    pub strength: u32,
    /// Direction of the last move; purely cosmetic.
    pub facing: Dir,
    /// Already moved, raised or founded this turn. Cleared when a turn starts.
    pub acted: bool,
}

#[derive(Clone, Debug)]
pub struct Player {
    pub name: String,
    pub alive: bool,
}

/// A production site. `Game::bases[i]` for `i < players.len()` is player
/// `i`'s home base; later entries were founded by settlers, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Base {
    pub pos: Pos,
    /// Who may produce here. Changes when another player's tank ends a move
    /// on it; `None` once the holder is out.
    pub holder: Option<usize>,
    /// Already produced this turn. Cleared when a turn starts.
    pub acted: bool,
}

/// What a player may do on their turn: each unit acts once (move, raise or
/// found), each held base produces once, then `Pass` ends the turn.
/// This is the only rules type that crosses the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Move {
        unit: u32,
        dir: Dir,
    },
    /// New unit at the held base on `at`, or at the first held base with room
    /// (home first) when `at` is `None`.
    Produce {
        kind: UnitKind,
        at: Option<Pos>,
    },
    Raise {
        unit: u32,
    },
    /// Turn a settler into a base on its tile.
    Found {
        unit: u32,
    },
    /// End the turn.
    Pass,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ActionError {
    GameOver,
    NoSuchUnit,
    NotYourUnit,
    OffMap,
    Water,
    OwnUnitThere,
    SettlerThere,
    NoBase,
    NotYourBase,
    NoRoomAtBase,
    CannotRaiseSettler,
    NotASettler,
    TooCloseToBase,
    UnitAlreadyActed,
    BaseAlreadyActed,
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ActionError::GameOver => "game is over",
            ActionError::NoSuchUnit => "no such unit",
            ActionError::NotYourUnit => "that tank is not yours",
            ActionError::OffMap => "cannot move off the map",
            ActionError::Water => "cannot drive into water",
            ActionError::OwnUnitThere => "one of your tanks is already there",
            ActionError::SettlerThere => "a settler is already there",
            ActionError::NoBase => "you hold no base to produce at",
            ActionError::NotYourBase => "you do not hold that base",
            ActionError::NoRoomAtBase => "no free cell at your bases",
            ActionError::CannotRaiseSettler => "settlers cannot be raised",
            ActionError::NotASettler => "only settlers can found a base",
            ActionError::TooCloseToBase => "too close to a base: keep two free tiles between bases",
            ActionError::UnitAlreadyActed => "that unit has already acted this turn",
            ActionError::BaseAlreadyActed => "that base has already produced this turn",
        };
        f.write_str(s)
    }
}

/// How a finished game ended.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum GameOver {
    Winner(usize),
    /// Everyone was eliminated at once.
    Tie,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Outcome {
    Moved,
    AttackerWon { remaining: u32 },
    DefenderWon { remaining: u32 },
    BothDestroyed,
    Produced { unit: u32, kind: UnitKind },
    Founded { base: usize },
    Raised { unit: u32, strength: u32 },
    Passed,
}

// ---------------------------------------------------------------------------
// Deterministic PRNG (splitmix64) so a seed reproduces a map exactly.
// ---------------------------------------------------------------------------

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform value in `0..n`. `n` must be positive.
    pub fn below(&mut self, n: i32) -> i32 {
        let n = u64::try_from(n).expect("below(n) needs n > 0");
        i32::try_from(self.next_u64() % n).expect("value below an i32 fits i32")
    }

    /// True with probability `p` (24-bit resolution).
    pub fn chance(&mut self, p: f64) -> bool {
        let r = u32::try_from(self.next_u64() >> 40).expect("24-bit value fits u32");
        f64::from(r) / f64::from(1u32 << 24) < p
    }
}

// ---------------------------------------------------------------------------
// Game state
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct Game {
    pub width: i32,
    pub height: i32,
    terrain: Vec<Terrain>,
    pub players: Vec<Player>,
    pub bases: Vec<Base>,
    pub units: Vec<Unit>,
    pub current: usize,
    pub turn: u32,
    pub result: Option<GameOver>,
    pub log: Vec<String>,
    next_unit_id: u32,
}

const WATER_CHANCE: f64 = 0.18;

impl Game {
    /// Generate a map and place `n_players` bases on it.
    pub fn new(width: i32, height: i32, n_players: usize, seed: u64) -> Game {
        assert!(width >= 4 && height >= 4, "map must be at least 4x4");
        assert!(n_players >= 2, "need at least two players");
        assert!(
            n_players * 5 <= area(width, height),
            "too many players for this map size"
        );

        let mut rng = Rng::new(seed);
        let mut attempt = 0;
        let (terrain, bases) = loop {
            attempt += 1;
            let mut terrain = gen_terrain(width, height, &mut rng, WATER_CHANCE);
            if let Some(bases) = place_bases(width, height, n_players, &mut rng) {
                // Keep every base and its neighbours drivable.
                for &b in &bases {
                    clear_around(&mut terrain, width, height, b);
                }
                if all_connected(&terrain, width, height, &bases) {
                    break (terrain, bases);
                }
            }
            if attempt > 500 {
                // Pathological seed/map size: fall back to an open field.
                let terrain = vec![Terrain::Plain; area(width, height)];
                let bases = place_bases(width, height, n_players, &mut rng)
                    .expect("open field always has room for bases");
                break (terrain, bases);
            }
        };

        let players = (0..n_players)
            .map(|i| Player {
                name: format!("Player {}", i + 1),
                alive: true,
            })
            .collect::<Vec<_>>();
        let bases = bases
            .iter()
            .enumerate()
            .map(|(i, &pos)| Base {
                pos,
                holder: Some(i),
                acted: false,
            })
            .collect::<Vec<_>>();

        let mut game = Game {
            width,
            height,
            terrain,
            players,
            bases,
            units: Vec::new(),
            current: 0,
            turn: 1,
            result: None,
            log: vec![format!("Map {}x{}, seed {}", width, height, seed)],
            next_unit_id: 1,
        };
        // Everyone starts with one tank on their base.
        for i in 0..n_players {
            let base = game.bases[i].pos;
            game.spawn_unit(i, base, 1);
        }
        game
    }

    // -- queries ------------------------------------------------------------

    pub fn in_bounds(&self, p: Pos) -> bool {
        p.x >= 0 && p.y >= 0 && p.x < self.width && p.y < self.height
    }

    pub fn terrain_at(&self, p: Pos) -> Terrain {
        self.terrain[idx(self.width, p)]
    }

    pub fn unit_at(&self, p: Pos) -> Option<&Unit> {
        self.units.iter().find(|u| u.pos == p)
    }

    pub fn unit(&self, id: u32) -> Option<&Unit> {
        self.units.iter().find(|u| u.id == id)
    }

    /// Index into `bases` of the base on `p`, if any.
    pub fn base_at(&self, p: Pos) -> Option<usize> {
        self.bases.iter().position(|b| b.pos == p)
    }

    /// Positions of every base `player` currently holds: home base first,
    /// then the rest in `bases` order.
    pub fn held_bases(&self, player: usize) -> impl Iterator<Item = Pos> + '_ {
        let home = self
            .bases
            .get(player)
            .filter(|b| b.holder == Some(player))
            .map(|b| b.pos);
        let others = self
            .bases
            .iter()
            .enumerate()
            .filter(move |&(i, b)| i != player && b.holder == Some(player))
            .map(|(_, b)| b.pos);
        home.into_iter().chain(others)
    }

    /// Whether a new base on `p` keeps `BASE_GAP` free tiles to every base.
    pub fn can_found_at(&self, p: Pos) -> bool {
        self.bases.iter().all(|b| b.pos.chebyshev(p) > BASE_GAP)
    }

    pub fn alive_players(&self) -> impl Iterator<Item = usize> + '_ {
        self.players
            .iter()
            .enumerate()
            .filter(|(_, p)| p.alive)
            .map(|(i, _)| i)
    }

    pub fn current_player(&self) -> &Player {
        &self.players[self.current]
    }

    // -- actions ------------------------------------------------------------

    /// Apply the current player's action. `Pass` hands the turn to the next
    /// living player; anything else spends the acting unit or base. On error
    /// nothing changes.
    pub fn apply(&mut self, action: Action) -> Result<Outcome, ActionError> {
        if self.result.is_some() {
            return Err(ActionError::GameOver);
        }
        let me = self.current;
        // Look up the acting unit's kind before it can be destroyed.
        let noun = match action {
            Action::Move { unit, .. } | Action::Raise { unit } | Action::Found { unit } => {
                self.unit(unit).map_or("unit", |u| u.kind.name())
            }
            Action::Produce { kind, .. } => kind.name(),
            Action::Pass => "unit",
        };
        let outcome = match action {
            Action::Pass => Outcome::Passed,
            Action::Produce { kind, at } => self.produce(me, kind, at)?,
            Action::Raise { unit } => self.raise(me, unit)?,
            Action::Move { unit, dir } => self.move_unit(me, unit, dir)?,
            Action::Found { unit } => self.found(me, unit)?,
        };
        self.log.push(self.describe(me, action, outcome, noun));
        self.check_victory();
        if action == Action::Pass {
            self.advance_turn();
        }
        Ok(outcome)
    }

    /// A unit of `me` that has not acted yet this turn.
    fn own_unit(&self, me: usize, id: u32) -> Result<&Unit, ActionError> {
        let u = self.unit(id).ok_or(ActionError::NoSuchUnit)?;
        if u.owner != me {
            return Err(ActionError::NotYourUnit);
        }
        if u.acted {
            return Err(ActionError::UnitAlreadyActed);
        }
        Ok(u)
    }

    fn unit_mut(&mut self, id: u32) -> &mut Unit {
        self.units
            .iter_mut()
            .find(|u| u.id == id)
            .expect("unit id checked by caller")
    }

    fn remove_unit(&mut self, id: u32) {
        self.units.retain(|u| u.id != id);
    }

    fn spawn_unit(&mut self, owner: usize, pos: Pos, strength: u32) -> u32 {
        self.spawn(UnitKind::Tank, owner, pos, strength)
    }

    fn spawn(&mut self, kind: UnitKind, owner: usize, pos: Pos, strength: u32) -> u32 {
        let id = self.next_unit_id;
        self.next_unit_id += 1;
        self.units.push(Unit {
            id,
            kind,
            owner,
            pos,
            strength,
            facing: Dir::Right,
            acted: false,
        });
        id
    }

    /// New unit on or next to the chosen held base, or the first held base
    /// (home first) that has not produced yet and has a free cell. Tanks start
    /// at strength 1, settlers at 0. New units wait for the next turn.
    fn produce(
        &mut self,
        me: usize,
        kind: UnitKind,
        at: Option<Pos>,
    ) -> Result<Outcome, ActionError> {
        let bases: Vec<Pos> = match at {
            Some(p) => {
                let held = self
                    .base_at(p)
                    .is_some_and(|i| self.bases[i].holder == Some(me));
                if !held {
                    return Err(ActionError::NotYourBase);
                }
                vec![p]
            }
            None => self.held_bases(me).collect(),
        };
        if bases.is_empty() {
            return Err(ActionError::NoBase);
        }
        let ready: Vec<Pos> = bases
            .into_iter()
            .filter(|&p| self.base_at(p).is_some_and(|i| !self.bases[i].acted))
            .collect();
        if ready.is_empty() {
            return Err(ActionError::BaseAlreadyActed);
        }
        let (base, spot) = ready
            .iter()
            .flat_map(|&base| {
                std::iter::once(base)
                    .chain(DIRS.iter().map(move |&d| base.step(d)))
                    .map(move |p| (base, p))
            })
            .find(|&(_, p)| {
                self.in_bounds(p)
                    && self.terrain_at(p) == Terrain::Plain
                    && self.unit_at(p).is_none()
            })
            .ok_or(ActionError::NoRoomAtBase)?;
        let strength = match kind {
            UnitKind::Tank => 1,
            UnitKind::Settler => 0,
        };
        let unit = self.spawn(kind, me, spot, strength);
        self.unit_mut(unit).acted = true;
        if let Some(i) = self.base_at(base) {
            self.bases[i].acted = true;
        }
        Ok(Outcome::Produced { unit, kind })
    }

    fn raise(&mut self, me: usize, id: u32) -> Result<Outcome, ActionError> {
        if self.own_unit(me, id)?.kind == UnitKind::Settler {
            return Err(ActionError::CannotRaiseSettler);
        }
        let u = self.unit_mut(id);
        u.strength += 1;
        u.acted = true;
        let strength = u.strength;
        Ok(Outcome::Raised { unit: id, strength })
    }

    fn move_unit(&mut self, me: usize, id: u32, dir: Dir) -> Result<Outcome, ActionError> {
        let (from, strength, kind) = {
            let u = self.own_unit(me, id)?;
            (u.pos, u.strength, u.kind)
        };
        let to = from.step(dir);
        if !self.in_bounds(to) {
            return Err(ActionError::OffMap);
        }
        if self.terrain_at(to) == Terrain::Water {
            return Err(ActionError::Water);
        }
        if self.unit_at(to).is_some_and(|u| u.owner == me) {
            return Err(ActionError::OwnUnitThere);
        }
        // Settlers never fight each other; the tile is simply taken.
        if kind == UnitKind::Settler
            && self
                .unit_at(to)
                .is_some_and(|u| u.kind == UnitKind::Settler)
        {
            return Err(ActionError::SettlerThere);
        }
        let u = self.unit_mut(id);
        u.facing = dir;
        u.acted = true;

        let defender = self.unit_at(to).map(|u| (u.id, u.owner, u.strength));
        let outcome = match defender {
            None => {
                self.unit_mut(id).pos = to;
                Outcome::Moved
            }
            Some((def_id, _, def_strength)) => match strength.cmp(&def_strength) {
                std::cmp::Ordering::Greater => {
                    self.remove_unit(def_id);
                    let a = self.unit_mut(id);
                    a.pos = to;
                    a.strength = strength - def_strength;
                    Outcome::AttackerWon {
                        remaining: strength - def_strength,
                    }
                }
                std::cmp::Ordering::Less => {
                    self.remove_unit(id);
                    self.unit_mut(def_id).strength = def_strength - strength;
                    Outcome::DefenderWon {
                        remaining: def_strength - strength,
                    }
                }
                std::cmp::Ordering::Equal => {
                    self.remove_unit(id);
                    self.remove_unit(def_id);
                    Outcome::BothDestroyed
                }
            },
        };

        // A tank ending its move on a base we do not hold takes it over.
        if kind == UnitKind::Tank && self.unit(id).is_some() {
            if let Some(base) = self.base_at(to) {
                if self.bases[base].holder != Some(me) {
                    self.capture(base, me);
                }
            }
        }
        Ok(outcome)
    }

    fn found(&mut self, me: usize, id: u32) -> Result<Outcome, ActionError> {
        let u = self.own_unit(me, id)?;
        if u.kind != UnitKind::Settler {
            return Err(ActionError::NotASettler);
        }
        let pos = u.pos;
        if !self.can_found_at(pos) {
            return Err(ActionError::TooCloseToBase);
        }
        self.remove_unit(id);
        // Founded this turn: it produces from the next one.
        self.bases.push(Base {
            pos,
            holder: Some(me),
            acted: true,
        });
        Ok(Outcome::Founded {
            base: self.bases.len() - 1,
        })
    }

    /// Take `player` out of the match (surrender or lost connection). Their
    /// tanks vanish; if it was their turn, play passes on. A no-op for players
    /// already out or once the game has a result.
    pub fn resign(&mut self, player: usize) {
        if self.result.is_some() || !self.players.get(player).is_some_and(|p| p.alive) {
            return;
        }
        self.players[player].alive = false;
        self.units.retain(|u| u.owner != player);
        self.release_bases(player);
        self.log
            .push(format!("{} left the game.", self.players[player].name));
        self.check_victory();
        if self.result.is_none() && self.current == player {
            self.advance_turn();
        }
    }

    fn capture(&mut self, base: usize, by: usize) {
        let from = self.bases[base].holder;
        self.bases[base].holder = Some(by);
        // A freshly taken base produces from the captor's next turn.
        self.bases[base].acted = true;
        let who = &self.players[by].name;
        let msg = match from {
            _ if base == by => format!("{who} took back their home base!"),
            Some(prev) => format!("{who} captured a base from {}!", self.players[prev].name),
            None => format!("{who} captured a neutral base!"),
        };
        self.log.push(msg);
    }

    /// Bases held by a player who is out become neutral.
    fn release_bases(&mut self, player: usize) {
        for b in &mut self.bases {
            if b.holder == Some(player) {
                b.holder = None;
            }
        }
    }

    /// A player with no units and no base to produce at is out. Then: one
    /// player left wins, nobody left is a tie.
    fn check_victory(&mut self) {
        for i in 0..self.players.len() {
            let has_units = self.units.iter().any(|u| u.owner == i);
            if self.players[i].alive && !has_units && self.held_bases(i).next().is_none() {
                self.players[i].alive = false;
                self.log.push(format!(
                    "{} has no units and no bases left and is out.",
                    self.players[i].name
                ));
            }
        }
        let alive: Vec<usize> = self.alive_players().collect();
        match alive[..] {
            [] => {
                self.result = Some(GameOver::Tie);
                self.log.push("Everyone is out. The game is a draw!".into());
            }
            [w] => {
                self.result = Some(GameOver::Winner(w));
                self.log
                    .push(format!("{} wins the game!", self.players[w].name));
            }
            _ => {}
        }
    }

    fn advance_turn(&mut self) {
        if self.result.is_some() {
            return;
        }
        let n = self.players.len();
        let prev = self.current;
        for step in 1..=n {
            let idx = (prev + step) % n;
            if self.players[idx].alive {
                if idx <= prev {
                    self.turn += 1;
                }
                self.current = idx;
                for u in &mut self.units {
                    u.acted = false;
                }
                for b in &mut self.bases {
                    b.acted = false;
                }
                return;
            }
        }
    }

    fn describe(&self, me: usize, action: Action, outcome: Outcome, noun: &str) -> String {
        let name = &self.players[me].name;
        match (action, outcome) {
            (_, Outcome::Passed) => format!("{name} ended their turn."),
            (_, Outcome::Produced { unit, kind }) => {
                format!("{name} produced {} #{unit}.", kind.name())
            }
            (_, Outcome::Raised { unit, strength }) => {
                format!("{name} raised {noun} #{unit} to strength {strength}.")
            }
            (Action::Found { unit }, Outcome::Founded { .. }) => {
                format!("{name}'s settler #{unit} founded a new base.")
            }
            (Action::Move { unit, .. }, Outcome::Moved) => format!("{name} moved {noun} #{unit}."),
            (Action::Move { unit, .. }, Outcome::AttackerWon { remaining }) => {
                format!("{name}'s {noun} #{unit} won the fight, {remaining} strength left.")
            }
            (Action::Move { unit, .. }, Outcome::DefenderWon { remaining }) => {
                format!("{name}'s {noun} #{unit} was destroyed; defender has {remaining} left.")
            }
            (Action::Move { unit, .. }, Outcome::BothDestroyed) => {
                format!("{name}'s {noun} #{unit} and the defender destroyed each other.")
            }
            _ => format!("{name} acted."),
        }
    }
}

// ---------------------------------------------------------------------------
// Map generation helpers
// ---------------------------------------------------------------------------

fn idx(width: i32, p: Pos) -> usize {
    usize::try_from(p.y * width + p.x).expect("position must be inside the map")
}

fn area(width: i32, height: i32) -> usize {
    usize::try_from(width * height).expect("map dimensions must be positive")
}

fn gen_terrain(width: i32, height: i32, rng: &mut Rng, water: f64) -> Vec<Terrain> {
    let mut t = vec![Terrain::Plain; area(width, height)];
    for cell in &mut t {
        if rng.chance(water) {
            *cell = Terrain::Water;
        }
    }
    // One smoothing pass: lone water becomes land, dense water fills in. Gives
    // lakes instead of noise.
    let before = t.clone();
    for y in 0..height {
        for x in 0..width {
            let p = Pos::new(x, y);
            let wet = DIRS
                .iter()
                .map(|&d| p.step(d))
                .filter(|&n| n.x >= 0 && n.y >= 0 && n.x < width && n.y < height)
                .filter(|&n| before[idx(width, n)] == Terrain::Water)
                .count();
            let here = &mut t[idx(width, p)];
            if *here == Terrain::Water && wet == 0 {
                *here = Terrain::Plain;
            } else if *here == Terrain::Plain && wet >= 3 {
                *here = Terrain::Water;
            }
        }
    }
    t
}

fn clear_around(t: &mut [Terrain], width: i32, height: i32, p: Pos) {
    for q in std::iter::once(p).chain(DIRS.iter().map(|&d| p.step(d))) {
        if q.x >= 0 && q.y >= 0 && q.x < width && q.y < height {
            t[idx(width, q)] = Terrain::Plain;
        }
    }
}

/// Pick `n` base positions spread as far apart as the map allows.
fn place_bases(width: i32, height: i32, n: usize, rng: &mut Rng) -> Option<Vec<Pos>> {
    let max_dist = width.midpoint(height);
    for min_dist in (2..=max_dist).rev() {
        let mut bases: Vec<Pos> = Vec::with_capacity(n);
        for _ in 0..400 {
            let candidate = Pos::new(rng.below(width), rng.below(height));
            if bases.iter().all(|&b| b.manhattan(candidate) >= min_dist) {
                bases.push(candidate);
                if bases.len() == n {
                    return Some(bases);
                }
            }
        }
    }
    None
}

fn all_connected(t: &[Terrain], width: i32, height: i32, bases: &[Pos]) -> bool {
    let mut seen = vec![false; t.len()];
    let mut queue = VecDeque::new();
    seen[idx(width, bases[0])] = true;
    queue.push_back(bases[0]);
    while let Some(p) = queue.pop_front() {
        for &d in &DIRS {
            let q = p.step(d);
            if q.x < 0 || q.y < 0 || q.x >= width || q.y >= height {
                continue;
            }
            let i = idx(width, q);
            if !seen[i] && t[i] == Terrain::Plain {
                seen[i] = true;
                queue.push_back(q);
            }
        }
    }
    bases.iter().all(|&b| seen[idx(width, b)])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Small open map with two players at known positions and no starting tanks.
    fn arena() -> Game {
        let mut g = Game::new(6, 6, 2, 42);
        g.terrain = vec![Terrain::Plain; 36];
        g.bases[0].pos = Pos::new(0, 0);
        g.bases[1].pos = Pos::new(5, 5);
        g.units.clear();
        g
    }

    #[test]
    fn new_game_places_one_tank_per_player_on_base() {
        let g = Game::new(16, 12, 4, 7);
        assert_eq!(g.players.len(), 4);
        assert_eq!(g.units.len(), 4);
        assert_eq!(g.bases.len(), 4);
        for (i, b) in g.bases.iter().enumerate() {
            let u = g.unit_at(b.pos).expect("tank on base");
            assert_eq!(u.owner, i);
            assert_eq!(b.holder, Some(i));
            assert_eq!(g.terrain_at(b.pos), Terrain::Plain);
        }
    }

    #[test]
    fn same_seed_same_map() {
        let a = Game::new(16, 12, 3, 99);
        let b = Game::new(16, 12, 3, 99);
        assert_eq!(a.terrain, b.terrain);
        assert_eq!(a.bases, b.bases);
    }

    #[test]
    fn bases_are_connected() {
        for seed in 0..50 {
            let g = Game::new(12, 10, 4, seed);
            let bases: Vec<Pos> = g.bases.iter().map(|b| b.pos).collect();
            assert!(
                all_connected(&g.terrain, g.width, g.height, &bases),
                "seed {seed}"
            );
        }
    }

    #[test]
    fn stronger_attacker_wins_and_keeps_difference() {
        let mut g = arena();
        let a = g.spawn_unit(0, Pos::new(2, 2), 5);
        let d = g.spawn_unit(1, Pos::new(3, 2), 2);
        g.spawn_unit(1, Pos::new(5, 5), 1); // keeps player 2 in the game
        let out = g
            .apply(Action::Move {
                unit: a,
                dir: Dir::Right,
            })
            .unwrap();
        assert_eq!(out, Outcome::AttackerWon { remaining: 3 });
        assert!(g.unit(d).is_none());
        let a = g.unit(a).unwrap();
        assert_eq!(a.pos, Pos::new(3, 2));
        assert_eq!(a.strength, 3);
        assert_eq!(g.current, 0, "moving does not end the turn");
        g.apply(Action::Pass).unwrap();
        assert_eq!(g.current, 1, "passing hands the turn on");
    }

    #[test]
    fn stronger_defender_wins_and_keeps_difference() {
        let mut g = arena();
        let a = g.spawn_unit(0, Pos::new(2, 2), 2);
        let d = g.spawn_unit(1, Pos::new(2, 3), 5);
        let out = g
            .apply(Action::Move {
                unit: a,
                dir: Dir::Up,
            })
            .unwrap();
        assert_eq!(out, Outcome::DefenderWon { remaining: 3 });
        assert!(g.unit(a).is_none());
        let d = g.unit(d).unwrap();
        assert_eq!(d.pos, Pos::new(2, 3));
        assert_eq!(d.strength, 3);
    }

    #[test]
    fn equal_strength_destroys_both() {
        let mut g = arena();
        no_bases(&mut g);
        let a = g.spawn_unit(0, Pos::new(2, 2), 4);
        g.spawn_unit(1, Pos::new(1, 2), 4);
        let out = g
            .apply(Action::Move {
                unit: a,
                dir: Dir::Left,
            })
            .unwrap();
        assert_eq!(out, Outcome::BothDestroyed);
        assert!(g.units.is_empty());
        assert_eq!(g.result, Some(GameOver::Tie), "last units, no bases");
        assert!(g.players.iter().all(|p| !p.alive));
    }

    #[test]
    fn losing_last_unit_without_base_eliminates_player() {
        let mut g = arena();
        no_bases(&mut g);
        let a = g.spawn_unit(0, Pos::new(2, 2), 1);
        g.spawn_unit(1, Pos::new(2, 3), 5);
        g.apply(Action::Move {
            unit: a,
            dir: Dir::Up,
        })
        .unwrap();
        assert!(!g.players[0].alive);
        assert_eq!(g.result, Some(GameOver::Winner(1)));
    }

    #[test]
    fn mutual_destruction_with_third_player_left_is_not_a_tie() {
        let mut g = Game::new(8, 8, 3, 3);
        g.terrain = vec![Terrain::Plain; 64];
        g.units.clear();
        no_bases(&mut g);
        let a = g.spawn_unit(0, Pos::new(1, 1), 2);
        g.spawn_unit(1, Pos::new(2, 1), 2);
        g.spawn_unit(2, Pos::new(6, 6), 1);
        g.apply(Action::Move {
            unit: a,
            dir: Dir::Right,
        })
        .unwrap();
        assert_eq!(g.result, Some(GameOver::Winner(2)));
    }

    #[test]
    fn cannot_move_onto_own_unit_water_or_off_map() {
        let mut g = arena();
        let a = g.spawn_unit(0, Pos::new(0, 0), 1);
        g.spawn_unit(0, Pos::new(1, 0), 1);
        g.terrain[idx(6, Pos::new(0, 1))] = Terrain::Water;
        assert_eq!(
            g.apply(Action::Move {
                unit: a,
                dir: Dir::Right
            }),
            Err(ActionError::OwnUnitThere)
        );
        assert_eq!(
            g.apply(Action::Move {
                unit: a,
                dir: Dir::Up
            }),
            Err(ActionError::Water)
        );
        assert_eq!(
            g.apply(Action::Move {
                unit: a,
                dir: Dir::Left
            }),
            Err(ActionError::OffMap)
        );
        assert_eq!(g.current, 0, "failed actions do not end the turn");
    }

    #[test]
    fn cannot_command_enemy_unit() {
        let mut g = arena();
        let enemy = g.spawn_unit(1, Pos::new(3, 3), 1);
        assert_eq!(
            g.apply(Action::Raise { unit: enemy }),
            Err(ActionError::NotYourUnit)
        );
    }

    #[test]
    fn produce_uses_base_then_neighbours() {
        let mut g = arena();
        g.spawn_unit(1, Pos::new(5, 5), 1); // keeps player 2 in the game
        let first = g
            .apply(Action::Produce {
                kind: UnitKind::Tank,
                at: None,
            })
            .unwrap();
        let Outcome::Produced { unit, .. } = first else {
            panic!()
        };
        assert_eq!(g.unit(unit).unwrap().pos, Pos::new(0, 0));
        g.apply(Action::Pass).unwrap();
        g.apply(Action::Pass).unwrap(); // player 2
        let Outcome::Produced { unit, .. } = g
            .apply(Action::Produce {
                kind: UnitKind::Tank,
                at: None,
            })
            .unwrap()
        else {
            panic!()
        };
        let p = g.unit(unit).unwrap().pos;
        assert_eq!(p.manhattan(Pos::new(0, 0)), 1);
    }

    #[test]
    fn move_updates_facing_but_blocked_move_does_not() {
        let mut g = arena();
        let a = g.spawn_unit(0, Pos::new(2, 2), 1);
        g.spawn_unit(0, Pos::new(2, 3), 1);
        assert_eq!(g.unit(a).unwrap().facing, Dir::Right);
        assert_eq!(
            g.apply(Action::Move {
                unit: a,
                dir: Dir::Up
            }),
            Err(ActionError::OwnUnitThere)
        );
        assert_eq!(g.unit(a).unwrap().facing, Dir::Right);
        g.apply(Action::Move {
            unit: a,
            dir: Dir::Left,
        })
        .unwrap();
        assert_eq!(g.unit(a).unwrap().facing, Dir::Left);
    }

    #[test]
    fn raise_adds_one_strength() {
        let mut g = arena();
        let a = g.spawn_unit(0, Pos::new(2, 2), 1);
        let out = g.apply(Action::Raise { unit: a }).unwrap();
        assert_eq!(
            out,
            Outcome::Raised {
                unit: a,
                strength: 2
            }
        );
        assert_eq!(g.unit(a).unwrap().strength, 2);
    }

    #[test]
    fn capturing_base_takes_production_but_owner_stays_in() {
        let mut g = arena();
        let a = g.spawn_unit(0, Pos::new(4, 5), 3);
        let far = g.spawn_unit(1, Pos::new(0, 5), 9); // enemy army elsewhere
        g.apply(Action::Move {
            unit: a,
            dir: Dir::Right,
        })
        .unwrap();
        assert_eq!(g.bases[1].holder, Some(0));
        assert!(g.players[1].alive, "still has a tank");
        assert!(g.unit(far).is_some());
        assert!(g.result.is_none());
        g.apply(Action::Pass).unwrap();
        assert_eq!(
            g.apply(Action::Produce {
                kind: UnitKind::Tank,
                at: None
            }),
            Err(ActionError::NoBase)
        );
        assert_eq!(g.current, 1, "failed produce keeps the turn");
    }

    #[test]
    fn captor_produces_at_home_first_then_captured_base() {
        let mut g = arena();
        g.bases[1].holder = Some(0);
        g.spawn_unit(1, Pos::new(3, 3), 1); // keeps player 2 in the game
                                            // Fill player 1's home and its neighbours.
        for p in [Pos::new(0, 0), Pos::new(1, 0), Pos::new(0, 1)] {
            g.spawn_unit(0, p, 1);
        }
        let Outcome::Produced { unit, .. } = g
            .apply(Action::Produce {
                kind: UnitKind::Tank,
                at: None,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(g.unit(unit).unwrap().pos, Pos::new(5, 5));
    }

    #[test]
    fn owner_can_retake_base() {
        let mut g = arena();
        g.bases[1].holder = Some(0);
        g.spawn_unit(0, Pos::new(0, 0), 1);
        let b = g.spawn_unit(1, Pos::new(5, 4), 1);
        g.apply(Action::Pass).unwrap();
        g.apply(Action::Move {
            unit: b,
            dir: Dir::Up,
        })
        .unwrap();
        assert_eq!(g.bases[1].holder, Some(1));
        assert!(g.log.iter().any(|l| l.contains("took back")));
    }

    #[test]
    fn resigning_frees_held_bases() {
        let mut g = Game::new(8, 8, 3, 3);
        g.bases[2].holder = Some(1);
        g.resign(1);
        assert_eq!(g.bases[1].holder, None);
        assert_eq!(g.bases[2].holder, None);
        assert_eq!(g.bases[0].holder, Some(0));
    }

    #[test]
    fn player_without_units_but_with_base_stays_in_and_produces() {
        let mut g = arena();
        let a = g.spawn_unit(0, Pos::new(2, 2), 3);
        g.spawn_unit(1, Pos::new(3, 2), 1);
        g.apply(Action::Move {
            unit: a,
            dir: Dir::Right,
        })
        .unwrap();
        assert!(g.players[1].alive, "still holds home base");
        assert!(g.result.is_none());
        g.apply(Action::Pass).unwrap();
        assert_eq!(g.current, 1);
        let Outcome::Produced { unit, .. } = g
            .apply(Action::Produce {
                kind: UnitKind::Tank,
                at: None,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(g.unit(unit).unwrap().pos, Pos::new(5, 5));
    }

    #[test]
    fn capturing_last_base_of_unitless_player_ends_game() {
        let mut g = arena();
        let a = g.spawn_unit(0, Pos::new(4, 5), 2);
        g.apply(Action::Move {
            unit: a,
            dir: Dir::Right,
        })
        .unwrap();
        assert!(!g.players[1].alive);
        assert_eq!(g.result, Some(GameOver::Winner(0)));
    }

    /// Make every base neutral so losing units is what decides elimination.
    fn no_bases(g: &mut Game) {
        for b in &mut g.bases {
            b.holder = None;
        }
    }

    fn settler(g: &mut Game, owner: usize, pos: Pos) -> u32 {
        g.spawn(UnitKind::Settler, owner, pos, 0)
    }

    #[test]
    fn produce_settler_has_zero_strength_and_cannot_be_raised() {
        let mut g = arena();
        g.spawn_unit(1, Pos::new(5, 5), 1);
        let Outcome::Produced { unit, kind } = g
            .apply(Action::Produce {
                kind: UnitKind::Settler,
                at: None,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(kind, UnitKind::Settler);
        assert_eq!(g.unit(unit).unwrap().strength, 0);
        g.apply(Action::Pass).unwrap();
        g.apply(Action::Pass).unwrap();
        assert_eq!(
            g.apply(Action::Raise { unit }),
            Err(ActionError::CannotRaiseSettler)
        );
    }

    #[test]
    fn produce_at_chosen_base_only_if_held() {
        let mut g = arena();
        g.spawn_unit(1, Pos::new(3, 3), 1);
        g.bases.push(Base {
            pos: Pos::new(3, 0),
            holder: Some(0),
            acted: false,
        });
        let Outcome::Produced { unit, .. } = g
            .apply(Action::Produce {
                kind: UnitKind::Tank,
                at: Some(Pos::new(3, 0)),
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(g.unit(unit).unwrap().pos, Pos::new(3, 0));
        g.apply(Action::Pass).unwrap();
        g.apply(Action::Pass).unwrap();
        assert_eq!(
            g.apply(Action::Produce {
                kind: UnitKind::Tank,
                at: Some(Pos::new(5, 5)),
            }),
            Err(ActionError::NotYourBase)
        );
    }

    #[test]
    fn settler_founds_base_and_is_consumed() {
        let mut g = arena();
        g.spawn_unit(1, Pos::new(5, 5), 1);
        let s = settler(&mut g, 0, Pos::new(3, 0));
        assert_eq!(
            g.apply(Action::Found { unit: s }),
            Ok(Outcome::Founded { base: 2 })
        );
        assert!(g.unit(s).is_none());
        assert_eq!(
            g.bases[2],
            Base {
                pos: Pos::new(3, 0),
                holder: Some(0),
                acted: true,
            }
        );
    }

    #[test]
    fn founding_needs_two_free_tiles_to_every_base() {
        let mut g = arena();
        // Home bases at (0,0) and (5,5).
        for (pos, ok) in [
            (Pos::new(2, 0), false),
            (Pos::new(2, 2), false),
            (Pos::new(3, 0), true),
            (Pos::new(3, 3), false),
            (Pos::new(2, 3), true),
            (Pos::new(3, 2), true),
        ] {
            assert_eq!(g.can_found_at(pos), ok, "{pos:?}");
        }
        let s = settler(&mut g, 0, Pos::new(2, 1));
        assert_eq!(
            g.apply(Action::Found { unit: s }),
            Err(ActionError::TooCloseToBase)
        );
        let t = g.spawn_unit(0, Pos::new(3, 0), 1);
        assert_eq!(
            g.apply(Action::Found { unit: t }),
            Err(ActionError::NotASettler)
        );
    }

    #[test]
    fn settler_blocked_by_settler_and_loses_to_tank() {
        let mut g = arena();
        let s = settler(&mut g, 0, Pos::new(2, 2));
        settler(&mut g, 1, Pos::new(3, 2));
        let tank = g.spawn_unit(1, Pos::new(2, 3), 2);
        assert_eq!(
            g.apply(Action::Move {
                unit: s,
                dir: Dir::Right
            }),
            Err(ActionError::SettlerThere)
        );
        assert_eq!(
            g.apply(Action::Move {
                unit: s,
                dir: Dir::Up
            }),
            Ok(Outcome::DefenderWon { remaining: 2 })
        );
        assert!(g.unit(s).is_none());
        assert_eq!(g.unit(tank).unwrap().strength, 2);
    }

    #[test]
    fn tank_destroys_settler_at_no_cost() {
        let mut g = arena();
        let a = g.spawn_unit(0, Pos::new(2, 2), 2);
        let s = settler(&mut g, 1, Pos::new(3, 2));
        g.spawn_unit(1, Pos::new(5, 5), 1);
        assert_eq!(
            g.apply(Action::Move {
                unit: a,
                dir: Dir::Right
            }),
            Ok(Outcome::AttackerWon { remaining: 2 })
        );
        assert!(g.unit(s).is_none());
    }

    #[test]
    fn settler_does_not_capture_bases() {
        let mut g = arena();
        let s = settler(&mut g, 0, Pos::new(4, 5));
        g.spawn_unit(1, Pos::new(0, 5), 1);
        g.apply(Action::Move {
            unit: s,
            dir: Dir::Right,
        })
        .unwrap();
        assert_eq!(g.bases[1].holder, Some(1));
    }

    #[test]
    fn player_with_only_settlers_stays_in() {
        let mut g = arena();
        let a = g.spawn_unit(0, Pos::new(2, 2), 3);
        g.spawn_unit(1, Pos::new(3, 2), 1);
        settler(&mut g, 1, Pos::new(5, 0));
        g.apply(Action::Move {
            unit: a,
            dir: Dir::Right,
        })
        .unwrap();
        assert!(g.players[1].alive);
        assert!(g.result.is_none());
    }

    #[test]
    fn each_unit_acts_once_per_turn() {
        let mut g = arena();
        let a = g.spawn_unit(0, Pos::new(2, 2), 1);
        let b = g.spawn_unit(0, Pos::new(3, 3), 1);
        g.spawn_unit(1, Pos::new(5, 0), 1);
        let right = Action::Move {
            unit: a,
            dir: Dir::Right,
        };
        g.apply(right).unwrap();
        assert_eq!(g.apply(right), Err(ActionError::UnitAlreadyActed));
        assert_eq!(
            g.apply(Action::Raise { unit: a }),
            Err(ActionError::UnitAlreadyActed)
        );
        g.apply(Action::Raise { unit: b }).unwrap();
        assert_eq!(g.current, 0);
        g.apply(Action::Pass).unwrap();
        g.apply(Action::Pass).unwrap();
        g.apply(right).unwrap();
        assert_eq!(g.unit(a).unwrap().pos, Pos::new(4, 2));
    }

    #[test]
    fn each_base_produces_once_per_turn_and_new_units_wait() {
        let mut g = arena();
        g.spawn_unit(1, Pos::new(3, 3), 1);
        g.bases.push(Base {
            pos: Pos::new(3, 0),
            holder: Some(0),
            acted: false,
        });
        let tank = Action::Produce {
            kind: UnitKind::Tank,
            at: None,
        };
        let Outcome::Produced { unit, .. } = g.apply(tank).unwrap() else {
            panic!()
        };
        assert_eq!(g.unit(unit).unwrap().pos, Pos::new(0, 0));
        assert_eq!(
            g.apply(Action::Raise { unit }),
            Err(ActionError::UnitAlreadyActed)
        );
        assert_eq!(
            g.apply(Action::Produce {
                kind: UnitKind::Tank,
                at: Some(Pos::new(0, 0)),
            }),
            Err(ActionError::BaseAlreadyActed)
        );
        let Outcome::Produced { unit: second, .. } = g.apply(tank).unwrap() else {
            panic!()
        };
        assert_eq!(g.unit(second).unwrap().pos, Pos::new(3, 0));
        assert_eq!(g.apply(tank), Err(ActionError::BaseAlreadyActed));
        g.apply(Action::Pass).unwrap();
        g.apply(Action::Pass).unwrap();
        g.apply(Action::Raise { unit }).unwrap();
        g.apply(tank).unwrap();
    }

    #[test]
    fn captured_base_produces_from_next_turn() {
        let mut g = arena();
        g.bases[0].holder = None;
        let a = g.spawn_unit(0, Pos::new(4, 5), 3);
        g.spawn_unit(1, Pos::new(0, 5), 1);
        g.apply(Action::Move {
            unit: a,
            dir: Dir::Right,
        })
        .unwrap();
        let tank = Action::Produce {
            kind: UnitKind::Tank,
            at: None,
        };
        assert_eq!(g.apply(tank), Err(ActionError::BaseAlreadyActed));
        g.apply(Action::Pass).unwrap();
        g.apply(Action::Pass).unwrap();
        assert!(g.apply(tank).is_ok());
    }

    #[test]
    fn turn_counter_skips_dead_players() {
        let mut g = Game::new(8, 8, 3, 1);
        g.players[1].alive = false;
        g.units.retain(|u| u.owner != 1);
        assert_eq!(g.current, 0);
        g.apply(Action::Pass).unwrap();
        assert_eq!(g.current, 2);
        assert_eq!(g.turn, 1);
        g.apply(Action::Pass).unwrap();
        assert_eq!(g.current, 0);
        assert_eq!(g.turn, 2);
    }

    #[test]
    fn resign_removes_tanks_and_passes_turn() {
        let mut g = Game::new(8, 8, 3, 5);
        assert_eq!(g.current, 0);
        g.resign(0);
        assert!(!g.players[0].alive);
        assert!(g.units.iter().all(|u| u.owner != 0));
        assert_eq!(g.current, 1);
        assert!(g.result.is_none());
    }

    #[test]
    fn resign_off_turn_keeps_current_player() {
        let mut g = Game::new(8, 8, 3, 5);
        g.resign(2);
        assert_eq!(g.current, 0);
        assert!(!g.players[2].alive);
    }

    #[test]
    fn resign_of_last_opponent_ends_game() {
        let mut g = Game::new(8, 8, 2, 5);
        g.resign(1);
        assert_eq!(g.result, Some(GameOver::Winner(0)));
        // Further resignations and actions are rejected.
        g.resign(0);
        assert_eq!(g.result, Some(GameOver::Winner(0)));
        assert_eq!(g.apply(Action::Pass), Err(ActionError::GameOver));
    }
}
