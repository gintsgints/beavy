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
    Explorer,
}

impl UnitKind {
    pub fn name(self) -> &'static str {
        match self {
            UnitKind::Tank => "tank",
            UnitKind::Explorer => "explorer",
        }
    }

    /// The name with its indefinite article, e.g. "an explorer".
    pub fn a_name(self) -> &'static str {
        match self {
            UnitKind::Tank => "a tank",
            UnitKind::Explorer => "an explorer",
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
/// `i`'s home base; later entries were founded by explorers, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Base {
    pub pos: Pos,
    /// Who may produce here. Changes when another player's tank ends a move
    /// on it; `None` once the holder is out.
    pub holder: Option<usize>,
    /// Already produced this turn. Cleared when a turn starts.
    pub acted: bool,
    /// The unit being built here, if any. Lost when the base changes hands.
    pub build: Option<Build>,
}

/// Turns from ordering a unit to it rolling out: ordered on turn N, it
/// appears at the start of its owner's turn N + `BUILD_TURNS`.
pub const BUILD_TURNS: u32 = 5;

/// A unit under construction at a base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Build {
    pub kind: UnitKind,
    /// Turn number at whose start (for the base's holder) the unit rolls out.
    pub ready: u32,
}

/// What a player may do on their turn: each unit acts once (move, raise or
/// found), each idle held base starts one build, then `Pass` ends the turn. The turn
/// also ends by itself once the player has nothing legal left to do.
/// This is the only rules type that crosses the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Move {
        unit: u32,
        dir: Dir,
    },
    /// Start building a unit at the held base on `at`, or at the first idle
    /// held base (home first) when `at` is `None`. It rolls out `BUILD_TURNS`
    /// turns later.
    Produce {
        kind: UnitKind,
        at: Option<Pos>,
    },
    Raise {
        unit: u32,
    },
    /// Turn an explorer into a base on its tile.
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
    ExplorerThere,
    NoBase,
    NotYourBase,
    BaseBusy,
    CannotRaiseExplorer,
    NotAExplorer,
    TooCloseToBase,
    UnitAlreadyActed,
    BaseAlreadyActed,
    NotNearOwnBase,
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
            ActionError::ExplorerThere => "an explorer is already there",
            ActionError::NoBase => "you hold no base to produce at",
            ActionError::NotYourBase => "you do not hold that base",
            ActionError::BaseBusy => "that base is still building",
            ActionError::CannotRaiseExplorer => "explorers cannot be raised",
            ActionError::NotAExplorer => "only explorers can found a base",
            ActionError::TooCloseToBase => "too close to a base: keep two free tiles between bases",
            ActionError::UnitAlreadyActed => "that unit has already acted this turn",
            ActionError::BaseAlreadyActed => "that base has already produced this turn",
            ActionError::NotNearOwnBase => "tanks can only be raised on or next to a base you hold",
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
    AttackerWon {
        remaining: u32,
    },
    DefenderWon {
        remaining: u32,
    },
    BothDestroyed,
    /// A build started; the unit rolls out at the start of turn `ready`.
    Ordered {
        kind: UnitKind,
        ready: u32,
    },
    Founded {
        base: usize,
    },
    Raised {
        unit: u32,
        strength: u32,
    },
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

#[derive(Debug, Clone)]
pub struct Game {
    pub width: i32,
    pub height: i32,
    terrain: Vec<Terrain>,
    pub players: Vec<Player>,
    pub bases: Vec<Base>,
    pub units: Vec<Unit>,
    pub current: usize,
    /// Who moved first; a new turn number starts each time play reaches them.
    pub first: usize,
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
                build: None,
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
            first: 0,
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

    /// Same as `new`, but `first` moves first (wrapped to the player count).
    /// Used for rematches so the opening move rotates.
    pub fn with_first_player(
        width: i32,
        height: i32,
        n_players: usize,
        seed: u64,
        first: usize,
    ) -> Game {
        let mut game = Game::new(width, height, n_players, seed);
        game.first = first % n_players;
        game.current = game.first;
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

    /// Whether `player` could start a build at the base on `p` right now:
    /// they hold it, it is not building, and it has not acted this turn.
    pub fn can_produce_at(&self, player: usize, p: Pos) -> bool {
        self.build_site(player, Some(p)).is_ok()
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
    /// living player; anything else spends the acting unit or base, and the
    /// turn passes too once nothing is left to do. On error nothing changes.
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
        } else if self.result.is_none() && !self.has_moves_left(me) {
            self.log.push(format!(
                "{} has nothing left to do; the turn passes.",
                self.players[me].name
            ));
            self.advance_turn();
        }
        Ok(outcome)
    }

    /// Whether `player` could still do anything this turn besides passing:
    /// a unit with a legal move, raise or found, or a base that can produce.
    pub fn has_moves_left(&self, player: usize) -> bool {
        self.build_site(player, None).is_ok()
            || self
                .units
                .iter()
                .filter(|u| u.owner == player && !u.acted)
                .any(|u| {
                    DIRS.iter()
                        .any(|&d| self.move_target(player, u.id, d).is_ok())
                        || self.check_raise(player, u.id).is_ok()
                        || self.check_found(player, u.id).is_ok()
                })
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

    /// Start a unit at the chosen held base, or the first idle held base
    /// (home first). It rolls out `BUILD_TURNS` turns from now.
    fn produce(
        &mut self,
        me: usize,
        kind: UnitKind,
        at: Option<Pos>,
    ) -> Result<Outcome, ActionError> {
        let i = self.build_site(me, at)?;
        let ready = self.turn + BUILD_TURNS;
        let base = &mut self.bases[i];
        base.acted = true;
        base.build = Some(Build { kind, ready });
        Ok(Outcome::Ordered { kind, ready })
    }

    /// Index of the base a new build would go to.
    fn build_site(&self, me: usize, at: Option<Pos>) -> Result<usize, ActionError> {
        let held: Vec<usize> = if let Some(p) = at {
            let i = self
                .base_at(p)
                .filter(|&i| self.bases[i].holder == Some(me))
                .ok_or(ActionError::NotYourBase)?;
            vec![i]
        } else {
            // Home base (index `me`) first, then the others in founding order.
            std::iter::once(me)
                .chain((0..self.bases.len()).filter(|&i| i != me))
                .filter(|&i| self.bases[i].holder == Some(me))
                .collect()
        };
        if held.is_empty() {
            return Err(ActionError::NoBase);
        }
        if let Some(&i) = held
            .iter()
            .find(|&&i| !self.bases[i].acted && self.bases[i].build.is_none())
        {
            return Ok(i);
        }
        if held.iter().all(|&i| self.bases[i].build.is_some()) {
            Err(ActionError::BaseBusy)
        } else {
            Err(ActionError::BaseAlreadyActed)
        }
    }

    /// Roll out every finished build of the player whose turn just started,
    /// on or next to its base. With no free cell it waits for a later turn.
    /// New units can act straight away.
    fn deliver_builds(&mut self) {
        let me = self.current;
        for i in 0..self.bases.len() {
            let base = self.bases[i];
            let Some(build) = base.build.filter(|b| b.ready <= self.turn) else {
                continue;
            };
            if base.holder != Some(me) {
                continue;
            }
            let spot = std::iter::once(base.pos)
                .chain(DIRS.iter().map(|&d| base.pos.step(d)))
                .find(|&p| {
                    self.in_bounds(p)
                        && self.terrain_at(p) == Terrain::Plain
                        && self.unit_at(p).is_none()
                });
            let name = self.players[me].name.clone();
            let kind = build.kind;
            let Some(spot) = spot else {
                self.log.push(format!(
                    "{name}'s {} waits for room at its base.",
                    kind.name()
                ));
                continue;
            };
            let strength = match kind {
                UnitKind::Tank => 1,
                UnitKind::Explorer => 0,
            };
            let unit = self.spawn(kind, me, spot, strength);
            self.bases[i].build = None;
            self.log
                .push(format!("{name}'s new {} #{unit} rolled out.", kind.name()));
        }
    }

    /// +1 strength for a tank on or orthogonally next to a base `me` holds.
    fn raise(&mut self, me: usize, id: u32) -> Result<Outcome, ActionError> {
        self.check_raise(me, id)?;
        let u = self.unit_mut(id);
        u.strength += 1;
        u.acted = true;
        let strength = u.strength;
        Ok(Outcome::Raised { unit: id, strength })
    }

    fn check_raise(&self, me: usize, id: u32) -> Result<(), ActionError> {
        let u = self.own_unit(me, id)?;
        if u.kind == UnitKind::Explorer {
            return Err(ActionError::CannotRaiseExplorer);
        }
        let pos = u.pos;
        if !self.held_bases(me).any(|b| b.manhattan(pos) <= 1) {
            return Err(ActionError::NotNearOwnBase);
        }
        Ok(())
    }

    fn move_unit(&mut self, me: usize, id: u32, dir: Dir) -> Result<Outcome, ActionError> {
        let to = self.move_target(me, id, dir)?;
        let u = self.unit_mut(id);
        u.facing = dir;
        u.acted = true;
        let (strength, kind) = (u.strength, u.kind);

        let defender = self.unit_at(to).map(|u| (u.id, u.strength));
        let outcome = match defender {
            None => {
                self.unit_mut(id).pos = to;
                Outcome::Moved
            }
            Some((def_id, def_strength)) => match strength.cmp(&def_strength) {
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

    /// Where unit `id` would end up moving `dir`, if that move is legal.
    fn move_target(&self, me: usize, id: u32, dir: Dir) -> Result<Pos, ActionError> {
        let u = self.own_unit(me, id)?;
        let to = u.pos.step(dir);
        if !self.in_bounds(to) {
            return Err(ActionError::OffMap);
        }
        if self.terrain_at(to) == Terrain::Water {
            return Err(ActionError::Water);
        }
        if self.unit_at(to).is_some_and(|t| t.owner == me) {
            return Err(ActionError::OwnUnitThere);
        }
        // Explorers never fight each other; the tile is simply taken.
        if u.kind == UnitKind::Explorer
            && self
                .unit_at(to)
                .is_some_and(|t| t.kind == UnitKind::Explorer)
        {
            return Err(ActionError::ExplorerThere);
        }
        Ok(to)
    }

    fn found(&mut self, me: usize, id: u32) -> Result<Outcome, ActionError> {
        let pos = self.check_found(me, id)?;
        self.remove_unit(id);
        // Founded this turn: it produces from the next one.
        self.bases.push(Base {
            pos,
            holder: Some(me),
            acted: true,
            build: None,
        });
        Ok(Outcome::Founded {
            base: self.bases.len() - 1,
        })
    }

    /// The tile explorer `id` would found a base on, if it may.
    fn check_found(&self, me: usize, id: u32) -> Result<Pos, ActionError> {
        let u = self.own_unit(me, id)?;
        if u.kind != UnitKind::Explorer {
            return Err(ActionError::NotAExplorer);
        }
        if !self.can_found_at(u.pos) {
            return Err(ActionError::TooCloseToBase);
        }
        Ok(u.pos)
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
        // A freshly taken base produces from the captor's next turn, and
        // whatever the previous holder was building there is lost.
        self.bases[base].acted = true;
        self.bases[base].build = None;
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
                b.build = None;
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

    /// Hand the turn on, skipping players who could do nothing at all. If
    /// nobody can act, the turn stays with the next player so they can pass.
    fn advance_turn(&mut self) {
        if self.result.is_some() {
            return;
        }
        self.next_player();
        let (next, turn) = (self.current, self.turn);
        let mut skipped = Vec::new();
        while !self.has_moves_left(self.current) {
            skipped.push(self.current);
            if skipped.len() >= self.alive_players().count() {
                (self.current, self.turn) = (next, turn);
                skipped.clear();
                break;
            }
            self.next_player();
        }
        for p in skipped {
            self.log.push(format!(
                "{} has nothing to do; the turn passes.",
                self.players[p].name
            ));
        }
    }

    /// The next living player in seat order takes the turn with fresh units
    /// and bases.
    fn next_player(&mut self) {
        let n = self.players.len();
        let prev = self.current;
        // Seat order counted from whoever moved first.
        let order = |i: usize| (i + n - self.first) % n;
        for step in 1..=n {
            let idx = (prev + step) % n;
            if self.players[idx].alive {
                if order(idx) <= order(prev) {
                    self.turn += 1;
                }
                self.current = idx;
                for u in &mut self.units {
                    u.acted = false;
                }
                for b in &mut self.bases {
                    b.acted = false;
                }
                self.deliver_builds();
                return;
            }
        }
    }

    fn describe(&self, me: usize, action: Action, outcome: Outcome, noun: &str) -> String {
        let name = &self.players[me].name;
        match (action, outcome) {
            (_, Outcome::Passed) => format!("{name} ended their turn."),
            (_, Outcome::Ordered { kind, ready }) => {
                format!(
                    "{name} started {}; it rolls out on turn {ready}.",
                    kind.a_name()
                )
            }
            (_, Outcome::Raised { unit, strength }) => {
                format!("{name} raised {noun} #{unit} to strength {strength}.")
            }
            (Action::Found { unit }, Outcome::Founded { .. }) => {
                format!("{name}'s explorer #{unit} founded a new base.")
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
    fn build_rolls_out_after_build_turns_on_base_then_next_to_it() {
        let mut g = arena();
        g.spawn_unit(1, Pos::new(5, 5), 1); // keeps player 2 in the game
        assert_eq!(
            g.apply(Action::Produce {
                kind: UnitKind::Tank,
                at: None,
            }),
            Ok(Outcome::Ordered {
                kind: UnitKind::Tank,
                ready: 1 + BUILD_TURNS,
            })
        );
        assert!(g.units.iter().all(|u| u.owner != 0), "nothing yet");
        assert_eq!(g.current, 1, "nothing left to do, so the turn passed");
        let first = roll_out(&mut g, 0);
        assert_eq!((g.current, g.turn), (0, 1 + BUILD_TURNS));
        let u = g.unit(first).unwrap();
        assert_eq!(u.pos, Pos::new(0, 0));
        assert!(!u.acted, "a new unit can act at once");
        // The first tank stays on the base, so the next one rolls out beside it.
        g.apply(Action::Produce {
            kind: UnitKind::Tank,
            at: None,
        })
        .unwrap();
        let second = roll_out(&mut g, 0);
        assert_eq!(g.unit(second).unwrap().pos.manhattan(Pos::new(0, 0)), 1);
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
        let a = g.spawn_unit(0, Pos::new(1, 0), 1);
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
    fn raise_only_on_or_next_to_held_base() {
        let mut g = arena();
        g.bases[1].holder = None;
        g.spawn_unit(1, Pos::new(3, 3), 1);
        let on = g.spawn_unit(0, Pos::new(0, 0), 1);
        let diagonal = g.spawn_unit(0, Pos::new(1, 1), 1);
        let far = g.spawn_unit(0, Pos::new(2, 0), 1);
        let neutral = g.spawn_unit(0, Pos::new(4, 5), 1);
        g.apply(Action::Raise { unit: on }).unwrap();
        for unit in [diagonal, far, neutral] {
            assert_eq!(
                g.apply(Action::Raise { unit }),
                Err(ActionError::NotNearOwnBase)
            );
        }
        assert_eq!(g.unit(far).unwrap().strength, 1);
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
    fn orders_go_home_first_then_to_other_idle_bases() {
        let mut g = arena();
        g.bases[1].holder = Some(0);
        g.spawn_unit(1, Pos::new(3, 3), 1); // keeps player 2 in the game
        g.spawn_unit(0, Pos::new(2, 2), 1); // keeps player 1's turn open
        for _ in 0..2 {
            g.apply(Action::Produce {
                kind: UnitKind::Tank,
                at: None,
            })
            .unwrap();
        }
        assert!(g.bases[0].build.is_some());
        assert!(g.bases[1].build.is_some());
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
        g.apply(Action::Produce {
            kind: UnitKind::Tank,
            at: None,
        })
        .unwrap();
        let unit = roll_out(&mut g, 1);
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

    /// After `me` ordered a unit: pass turns until it rolls out at the start
    /// of their turn, and return its id.
    fn roll_out(g: &mut Game, me: usize) -> u32 {
        let before = g.units.iter().map(|u| u.id).max().unwrap_or(0);
        for _ in 0..20 {
            g.apply(Action::Pass).unwrap();
            if let Some(u) = g.units.iter().find(|u| u.owner == me && u.id > before) {
                assert_eq!(g.current, me, "rolls out at the start of the owner's turn");
                return u.id;
            }
        }
        panic!("build never rolled out");
    }

    /// Make every base neutral so losing units is what decides elimination.
    fn no_bases(g: &mut Game) {
        for b in &mut g.bases {
            b.holder = None;
        }
    }

    fn explorer(g: &mut Game, owner: usize, pos: Pos) -> u32 {
        g.spawn(UnitKind::Explorer, owner, pos, 0)
    }

    #[test]
    fn produce_explorer_has_zero_strength_and_cannot_be_raised() {
        let mut g = arena();
        g.spawn_unit(1, Pos::new(5, 5), 1);
        g.apply(Action::Produce {
            kind: UnitKind::Explorer,
            at: None,
        })
        .unwrap();
        let unit = roll_out(&mut g, 0);
        let u = g.unit(unit).unwrap();
        assert_eq!((u.kind, u.strength), (UnitKind::Explorer, 0));
        assert_eq!(
            g.apply(Action::Raise { unit }),
            Err(ActionError::CannotRaiseExplorer)
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
            build: None,
        });
        g.apply(Action::Produce {
            kind: UnitKind::Tank,
            at: Some(Pos::new(3, 0)),
        })
        .unwrap();
        assert!(g.bases[2].build.is_some());
        assert!(g.bases[0].build.is_none(), "home base untouched");
        assert_eq!(
            g.apply(Action::Produce {
                kind: UnitKind::Tank,
                at: Some(Pos::new(5, 5)),
            }),
            Err(ActionError::NotYourBase)
        );
    }

    #[test]
    fn explorer_founds_base_and_is_consumed() {
        let mut g = arena();
        g.spawn_unit(1, Pos::new(5, 5), 1);
        let s = explorer(&mut g, 0, Pos::new(3, 0));
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
                build: None,
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
        let s = explorer(&mut g, 0, Pos::new(2, 1));
        assert_eq!(
            g.apply(Action::Found { unit: s }),
            Err(ActionError::TooCloseToBase)
        );
        let t = g.spawn_unit(0, Pos::new(3, 0), 1);
        assert_eq!(
            g.apply(Action::Found { unit: t }),
            Err(ActionError::NotAExplorer)
        );
    }

    #[test]
    fn explorer_blocked_by_explorer_and_loses_to_tank() {
        let mut g = arena();
        let s = explorer(&mut g, 0, Pos::new(2, 2));
        explorer(&mut g, 1, Pos::new(3, 2));
        let tank = g.spawn_unit(1, Pos::new(2, 3), 2);
        assert_eq!(
            g.apply(Action::Move {
                unit: s,
                dir: Dir::Right
            }),
            Err(ActionError::ExplorerThere)
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
    fn tank_destroys_explorer_at_no_cost() {
        let mut g = arena();
        let a = g.spawn_unit(0, Pos::new(2, 2), 2);
        let s = explorer(&mut g, 1, Pos::new(3, 2));
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
    fn explorer_does_not_capture_bases() {
        let mut g = arena();
        let s = explorer(&mut g, 0, Pos::new(4, 5));
        g.spawn_unit(1, Pos::new(0, 5), 1);
        g.apply(Action::Move {
            unit: s,
            dir: Dir::Right,
        })
        .unwrap();
        assert_eq!(g.bases[1].holder, Some(1));
    }

    #[test]
    fn player_with_only_explorers_stays_in() {
        let mut g = arena();
        let a = g.spawn_unit(0, Pos::new(2, 2), 3);
        g.spawn_unit(1, Pos::new(3, 2), 1);
        explorer(&mut g, 1, Pos::new(5, 0));
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
        let b = g.spawn_unit(0, Pos::new(0, 1), 1);
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
    fn each_base_builds_one_unit_at_a_time() {
        let mut g = arena();
        g.spawn_unit(1, Pos::new(3, 3), 1);
        g.spawn_unit(0, Pos::new(2, 4), 1); // keeps player 1's turn open
        g.bases.push(Base {
            pos: Pos::new(3, 0),
            holder: Some(0),
            acted: false,
            build: None,
        });
        let tank = Action::Produce {
            kind: UnitKind::Tank,
            at: None,
        };
        let home = Action::Produce {
            kind: UnitKind::Tank,
            at: Some(Pos::new(0, 0)),
        };
        g.apply(tank).unwrap();
        assert_eq!(g.apply(home), Err(ActionError::BaseBusy));
        g.apply(tank).unwrap();
        assert!(
            g.bases[2].build.is_some(),
            "second order went to the other base"
        );
        assert_eq!(g.apply(tank), Err(ActionError::BaseBusy));
        g.apply(Action::Pass).unwrap();
        g.apply(Action::Pass).unwrap();
        assert_eq!(g.apply(home), Err(ActionError::BaseBusy), "still building");
        roll_out(&mut g, 0);
        assert_eq!(g.units.iter().filter(|u| u.owner == 0).count(), 3);
        g.apply(home).unwrap();
    }

    #[test]
    fn can_produce_at_tracks_holder_use_and_build() {
        let mut g = arena();
        g.spawn_unit(1, Pos::new(3, 3), 1);
        g.spawn_unit(0, Pos::new(2, 4), 1); // keeps player 1's turn open
        let home = Pos::new(0, 0);
        assert!(g.can_produce_at(0, home));
        assert!(!g.can_produce_at(0, Pos::new(5, 5)), "not held");
        assert!(!g.can_produce_at(0, Pos::new(2, 2)), "no base there");
        g.apply(Action::Produce {
            kind: UnitKind::Tank,
            at: Some(home),
        })
        .unwrap();
        assert!(!g.can_produce_at(0, home), "already ordered");
        g.apply(Action::Pass).unwrap();
        g.apply(Action::Pass).unwrap();
        assert!(!g.can_produce_at(0, home), "still building");

        // A crowded base can still take an order; the unit waits for room.
        let mut full = arena();
        for p in [home, Pos::new(1, 0), Pos::new(0, 1)] {
            full.spawn_unit(1, p, 1);
        }
        assert!(full.can_produce_at(0, home));
    }

    #[test]
    fn finished_build_waits_for_a_free_cell() {
        let mut g = arena();
        g.spawn_unit(1, Pos::new(5, 0), 1);
        let blockers: Vec<u32> = [Pos::new(0, 0), Pos::new(1, 0), Pos::new(0, 1)]
            .into_iter()
            .map(|p| g.spawn_unit(0, p, 1))
            .collect();
        g.bases[0].build = Some(Build {
            kind: UnitKind::Tank,
            ready: 2,
        });
        g.apply(Action::Pass).unwrap();
        g.apply(Action::Pass).unwrap();
        assert_eq!(g.turn, 2);
        assert!(g.bases[0].build.is_some(), "no room yet");
        assert!(g.log.iter().any(|l| l.contains("waits for room")));
        g.apply(Action::Move {
            unit: blockers[1],
            dir: Dir::Right,
        })
        .unwrap();
        g.apply(Action::Pass).unwrap();
        g.apply(Action::Pass).unwrap();
        assert!(g.bases[0].build.is_none());
        assert_eq!(g.unit_at(Pos::new(1, 0)).unwrap().owner, 0);
        assert_eq!(g.units.iter().filter(|u| u.owner == 0).count(), 4);
    }

    #[test]
    fn capturing_a_base_loses_its_build() {
        let mut g = arena();
        g.bases[1].build = Some(Build {
            kind: UnitKind::Tank,
            ready: 3,
        });
        let a = g.spawn_unit(0, Pos::new(4, 5), 3);
        g.spawn_unit(1, Pos::new(0, 5), 1);
        g.apply(Action::Move {
            unit: a,
            dir: Dir::Right,
        })
        .unwrap();
        assert_eq!(g.bases[1].holder, Some(0));
        assert!(g.bases[1].build.is_none());
    }

    #[test]
    fn captured_base_produces_from_next_turn() {
        let mut g = arena();
        g.bases[0].holder = None;
        let a = g.spawn_unit(0, Pos::new(4, 5), 3);
        g.spawn_unit(0, Pos::new(2, 2), 1); // keeps player 1's turn open
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
    fn turn_passes_by_itself_when_nothing_is_left() {
        let mut g = arena();
        g.spawn_unit(1, Pos::new(3, 3), 1);
        let a = g.spawn_unit(0, Pos::new(2, 0), 1);
        let right = Action::Move {
            unit: a,
            dir: Dir::Right,
        };
        g.apply(right).unwrap();
        assert_eq!(g.current, 0, "home base can still produce");
        g.apply(Action::Produce {
            kind: UnitKind::Tank,
            at: None,
        })
        .unwrap();
        assert_eq!(g.current, 1);
        assert!(g.log.iter().any(|l| l.contains("nothing left to do")));
    }

    #[test]
    fn player_with_nothing_to_do_at_turn_start_is_skipped() {
        let mut g = arena();
        // Player 2 lost their base and their only tank is boxed in by water.
        g.bases[1].holder = Some(0);
        let stuck = g.spawn_unit(1, Pos::new(5, 0), 1);
        g.terrain[idx(6, Pos::new(4, 0))] = Terrain::Water;
        g.terrain[idx(6, Pos::new(5, 1))] = Terrain::Water;
        g.spawn_unit(0, Pos::new(2, 2), 1);
        assert!(!g.has_moves_left(1));
        g.apply(Action::Pass).unwrap();
        assert_eq!((g.current, g.turn), (0, 2), "player 2 was skipped");
        assert!(g.players[1].alive, "skipped, not eliminated");
        assert!(g.unit(stuck).is_some());
        assert!(g
            .log
            .iter()
            .any(|l| l == "Player 2 has nothing to do; the turn passes."));
    }

    #[test]
    fn everyone_stuck_leaves_the_turn_with_the_next_player() {
        let mut g = arena();
        no_bases(&mut g);
        for (owner, pos, water) in [
            (0, Pos::new(0, 0), [Pos::new(1, 0), Pos::new(0, 1)]),
            (1, Pos::new(5, 5), [Pos::new(4, 5), Pos::new(5, 4)]),
        ] {
            g.spawn_unit(owner, pos, 1);
            for w in water {
                g.terrain[idx(6, w)] = Terrain::Water;
            }
        }
        g.apply(Action::Pass).unwrap();
        assert_eq!(g.current, 1, "no endless skipping when nobody can act");
    }

    #[test]
    fn boxed_in_units_and_busy_bases_count_as_nothing_left() {
        let mut g = arena();
        g.spawn_unit(1, Pos::new(3, 3), 1);
        // Home (0,0) is busy building and both free neighbours are taken by
        // player 1's units.
        g.bases[0].build = Some(Build {
            kind: UnitKind::Tank,
            ready: 9,
        });
        let a = g.spawn_unit(0, Pos::new(0, 0), 1);
        g.spawn_unit(0, Pos::new(1, 0), 1);
        g.spawn_unit(0, Pos::new(0, 1), 1);
        for u in &mut g.units {
            if u.owner == 0 && u.id != a {
                u.acted = true;
            }
        }
        assert!(g.has_moves_left(0), "the tank on the base can still raise");
        g.apply(Action::Raise { unit: a }).unwrap();
        assert_eq!(g.current, 1);
    }

    /// Every non-pass action the current player could try right now.
    fn candidate_actions(g: &Game) -> Vec<Action> {
        let me = g.current;
        let mut out = Vec::new();
        for u in g.units.iter().filter(|u| u.owner == me) {
            for dir in DIRS {
                out.push(Action::Move { unit: u.id, dir });
            }
            out.push(Action::Raise { unit: u.id });
            out.push(Action::Found { unit: u.id });
        }
        for kind in [UnitKind::Tank, UnitKind::Explorer] {
            out.push(Action::Produce { kind, at: None });
            for at in g.held_bases(me) {
                out.push(Action::Produce { kind, at: Some(at) });
            }
        }
        out
    }

    /// Random games: `has_moves_left` must agree with whether some action
    /// really succeeds, and play never waits on a player who cannot act.
    #[test]
    fn turn_never_waits_on_a_player_with_nothing_to_do() {
        // Small crowded boards, so bases fill up and units get boxed in.
        for seed in 0..30 {
            let players = 2 + usize::try_from(seed % 3).unwrap();
            let mut g = Game::new(6, 6, players, seed);
            let mut rng = Rng::new(seed);
            for step in 0..300 {
                if g.result.is_some() {
                    break;
                }
                let legal: Vec<Action> = candidate_actions(&g)
                    .into_iter()
                    .filter(|&a| g.clone().apply(a).is_ok())
                    .collect();
                assert_eq!(
                    g.has_moves_left(g.current),
                    !legal.is_empty(),
                    "seed {seed} step {step}: has_moves_left disagrees with the rules"
                );
                let nobody_can_act = g.alive_players().all(|p| !g.has_moves_left(p));
                assert!(
                    !legal.is_empty() || nobody_can_act,
                    "seed {seed} step {step}: {} has nothing to do but still has the turn",
                    g.current_player().name
                );
                // Mostly act, sometimes end the turn early.
                let action = if legal.is_empty() || rng.chance(0.1) {
                    Action::Pass
                } else {
                    let n = i32::try_from(legal.len()).unwrap();
                    legal[usize::try_from(rng.below(n)).unwrap()]
                };
                g.apply(action).unwrap();
            }
        }
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
    fn rematch_rotates_first_player_and_turn_counter() {
        let a = Game::new(8, 8, 3, 4);
        let mut g = Game::with_first_player(8, 8, 3, 4, 4);
        assert_eq!(g.terrain, a.terrain, "same seed, same map");
        assert_eq!(g.bases, a.bases);
        assert_eq!(g.current, 1);
        for expected in [2, 0] {
            g.apply(Action::Pass).unwrap();
            assert_eq!((g.current, g.turn), (expected, 1));
        }
        g.apply(Action::Pass).unwrap();
        assert_eq!((g.current, g.turn), (1, 2));
    }

    #[test]
    fn turn_counter_follows_first_player_when_they_are_out() {
        let mut g = Game::with_first_player(8, 8, 3, 4, 1);
        g.resign(1);
        assert_eq!((g.current, g.turn), (2, 1));
        g.apply(Action::Pass).unwrap();
        assert_eq!((g.current, g.turn), (0, 1));
        g.apply(Action::Pass).unwrap();
        assert_eq!((g.current, g.turn), (2, 2));
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
