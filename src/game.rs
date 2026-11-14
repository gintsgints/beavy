//! Pure game rules. No Bevy dependency, so the logic is unit-testable and can be
//! reused unchanged by a networked server later.

use std::collections::VecDeque;

// ---------------------------------------------------------------------------
// Basic types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Terrain {
    Plain,
    Water,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
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
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

pub const DIRS: [Dir; 4] = [Dir::Up, Dir::Down, Dir::Left, Dir::Right];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub id: u32,
    pub owner: usize,
    pub pos: Pos,
    pub strength: u32,
    /// Direction of the last move; purely cosmetic.
    pub facing: Dir,
}

#[derive(Clone, Debug)]
pub struct Player {
    pub name: String,
    pub base: Pos,
    pub alive: bool,
}

/// What a player may do on their turn. Exactly one action ends the turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Move { unit: u32, dir: Dir },
    Produce,
    Raise { unit: u32 },
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
    NoRoomAtBase,
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
            ActionError::NoRoomAtBase => "no free cell at your base",
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
    Produced { unit: u32 },
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

        let players = bases
            .iter()
            .enumerate()
            .map(|(i, &base)| Player {
                name: format!("Player {}", i + 1),
                base,
                alive: true,
            })
            .collect::<Vec<_>>();

        let mut game = Game {
            width,
            height,
            terrain,
            players,
            units: Vec::new(),
            current: 0,
            turn: 1,
            result: None,
            log: vec![format!("Map {}x{}, seed {}", width, height, seed)],
            next_unit_id: 1,
        };
        // Everyone starts with one tank on their base.
        for i in 0..n_players {
            let base = game.players[i].base;
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

    pub fn base_owner(&self, p: Pos) -> Option<usize> {
        self.players.iter().position(|pl| pl.base == p)
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

    /// Apply the current player's action. On success the turn passes to the
    /// next living player. On error nothing changes and it is still their turn.
    pub fn apply(&mut self, action: Action) -> Result<Outcome, ActionError> {
        if self.result.is_some() {
            return Err(ActionError::GameOver);
        }
        let me = self.current;
        let outcome = match action {
            Action::Pass => Outcome::Passed,
            Action::Produce => self.produce(me)?,
            Action::Raise { unit } => self.raise(me, unit)?,
            Action::Move { unit, dir } => self.move_unit(me, unit, dir)?,
        };
        self.log.push(self.describe(me, action, outcome));
        self.check_victory();
        self.advance_turn();
        Ok(outcome)
    }

    fn own_unit(&self, me: usize, id: u32) -> Result<&Unit, ActionError> {
        let u = self.unit(id).ok_or(ActionError::NoSuchUnit)?;
        if u.owner != me {
            return Err(ActionError::NotYourUnit);
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
        let id = self.next_unit_id;
        self.next_unit_id += 1;
        self.units.push(Unit {
            id,
            owner,
            pos,
            strength,
            facing: Dir::Right,
        });
        id
    }

    fn produce(&mut self, me: usize) -> Result<Outcome, ActionError> {
        let base = self.players[me].base;
        let spot = std::iter::once(base)
            .chain(DIRS.iter().map(|&d| base.step(d)))
            .find(|&p| {
                self.in_bounds(p)
                    && self.terrain_at(p) == Terrain::Plain
                    && self.unit_at(p).is_none()
            })
            .ok_or(ActionError::NoRoomAtBase)?;
        let unit = self.spawn_unit(me, spot, 1);
        Ok(Outcome::Produced { unit })
    }

    fn raise(&mut self, me: usize, id: u32) -> Result<Outcome, ActionError> {
        self.own_unit(me, id)?;
        let u = self.unit_mut(id);
        u.strength += 1;
        let strength = u.strength;
        Ok(Outcome::Raised { unit: id, strength })
    }

    fn move_unit(&mut self, me: usize, id: u32, dir: Dir) -> Result<Outcome, ActionError> {
        let (from, strength) = {
            let u = self.own_unit(me, id)?;
            (u.pos, u.strength)
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
        self.unit_mut(id).facing = dir;

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

        // Standing on an enemy base captures it and eliminates its owner.
        if self.unit(id).is_some() {
            if let Some(victim) = self.base_owner(to) {
                if victim != me && self.players[victim].alive {
                    self.eliminate(victim, me);
                }
            }
        }
        Ok(outcome)
    }

    fn eliminate(&mut self, victim: usize, by: usize) {
        self.players[victim].alive = false;
        self.units.retain(|u| u.owner != victim);
        self.log.push(format!(
            "{} captured {}'s base! {} is out.",
            self.players[by].name, self.players[victim].name, self.players[victim].name
        ));
    }

    /// A player with no tanks left is out. Then: one player left wins, nobody
    /// left is a tie.
    fn check_victory(&mut self) {
        for i in 0..self.players.len() {
            if self.players[i].alive && !self.units.iter().any(|u| u.owner == i) {
                self.players[i].alive = false;
                self.log.push(format!(
                    "{} has no tanks left and is out.",
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
                return;
            }
        }
    }

    fn describe(&self, me: usize, action: Action, outcome: Outcome) -> String {
        let name = &self.players[me].name;
        match (action, outcome) {
            (_, Outcome::Passed) => format!("{name} passed."),
            (_, Outcome::Produced { unit }) => format!("{name} produced tank #{unit}."),
            (_, Outcome::Raised { unit, strength }) => {
                format!("{name} raised tank #{unit} to strength {strength}.")
            }
            (Action::Move { unit, .. }, Outcome::Moved) => format!("{name} moved tank #{unit}."),
            (Action::Move { unit, .. }, Outcome::AttackerWon { remaining }) => {
                format!("{name}'s tank #{unit} won the fight, {remaining} strength left.")
            }
            (Action::Move { unit, .. }, Outcome::DefenderWon { remaining }) => {
                format!("{name}'s tank #{unit} was destroyed; defender has {remaining} left.")
            }
            (Action::Move { unit, .. }, Outcome::BothDestroyed) => {
                format!("{name}'s tank #{unit} and the defender destroyed each other.")
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
        g.players[0].base = Pos::new(0, 0);
        g.players[1].base = Pos::new(5, 5);
        g.units.clear();
        g
    }

    #[test]
    fn new_game_places_one_tank_per_player_on_base() {
        let g = Game::new(16, 12, 4, 7);
        assert_eq!(g.players.len(), 4);
        assert_eq!(g.units.len(), 4);
        for (i, p) in g.players.iter().enumerate() {
            let u = g.unit_at(p.base).expect("tank on base");
            assert_eq!(u.owner, i);
            assert_eq!(g.terrain_at(p.base), Terrain::Plain);
        }
    }

    #[test]
    fn same_seed_same_map() {
        let a = Game::new(16, 12, 3, 99);
        let b = Game::new(16, 12, 3, 99);
        assert_eq!(a.terrain, b.terrain);
        assert_eq!(
            a.players.iter().map(|p| p.base).collect::<Vec<_>>(),
            b.players.iter().map(|p| p.base).collect::<Vec<_>>()
        );
    }

    #[test]
    fn bases_are_connected() {
        for seed in 0..50 {
            let g = Game::new(12, 10, 4, seed);
            let bases: Vec<Pos> = g.players.iter().map(|p| p.base).collect();
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
        assert_eq!(g.current, 1, "turn passed to next player");
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
        assert_eq!(g.result, Some(GameOver::Tie), "last tanks of both players");
        assert!(g.players.iter().all(|p| !p.alive));
    }

    #[test]
    fn losing_last_tank_eliminates_player_and_other_wins() {
        let mut g = arena();
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
        let first = g.apply(Action::Produce).unwrap();
        let Outcome::Produced { unit } = first else {
            panic!()
        };
        assert_eq!(g.unit(unit).unwrap().pos, Pos::new(0, 0));
        g.apply(Action::Pass).unwrap(); // player 2
        let Outcome::Produced { unit } = g.apply(Action::Produce).unwrap() else {
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
    fn capturing_base_eliminates_owner_and_ends_game() {
        let mut g = arena();
        let a = g.spawn_unit(0, Pos::new(4, 5), 3);
        g.spawn_unit(1, Pos::new(0, 5), 9); // enemy army elsewhere
        g.apply(Action::Move {
            unit: a,
            dir: Dir::Right,
        })
        .unwrap();
        assert!(!g.players[1].alive);
        assert!(
            g.units.iter().all(|u| u.owner == 0),
            "victim's units removed"
        );
        assert_eq!(g.result, Some(GameOver::Winner(0)));
        assert_eq!(g.apply(Action::Pass), Err(ActionError::GameOver));
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
}
