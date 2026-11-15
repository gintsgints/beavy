//! Online play: the wire protocol, room bookkeeping and the client-side socket
//! wrapper. No Bevy here. `server.rs` drives `Lobby` over TCP; `main.rs` talks
//! to it through `Client`.
//!
//! The model is lockstep: the server owns one `Game` per room and only
//! broadcasts accepted `Action`s. Every client rebuilds the same board from the
//! shared seed and replays the actions, so board state never crosses the wire.

use crate::game::{Action, ActionError, Game, Outcome};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

pub const DEFAULT_ADDR: &str = "127.0.0.1:7777";
pub const MAX_NAME_LEN: usize = 16;
pub const MAX_CODE_LEN: usize = 8;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);

// ---------------------------------------------------------------------------
// Protocol
// ---------------------------------------------------------------------------

/// Everything needed to rebuild a board identically on every machine.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchSettings {
    pub width: i32,
    pub height: i32,
    pub players: usize,
    pub seed: u64,
}

impl MatchSettings {
    /// Reject anything the client menu would not allow, so a hostile or buggy
    /// client cannot make `Game::new` panic on the server.
    pub fn validate(&self) -> Result<(), String> {
        let (min_w, max_w) = (crate::MIN_W, crate::MAX_W);
        let (min_h, max_h) = (crate::MIN_H, crate::MAX_H);
        if !(min_w..=max_w).contains(&self.width) || !(min_h..=max_h).contains(&self.height) {
            return Err(format!(
                "board must be {min_w}-{max_w} wide and {min_h}-{max_h} high"
            ));
        }
        let by_area = usize::try_from(self.width * self.height / 5).unwrap_or(0);
        let max = crate::MAX_PLAYERS.min(by_area);
        if !(crate::MIN_PLAYERS..=max).contains(&self.players) {
            return Err(format!(
                "players must be {}-{max} for this board",
                crate::MIN_PLAYERS
            ));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ClientMsg {
    /// Open a room under the host's chosen `code` and take seat 0.
    Create {
        code: String,
        name: String,
        settings: MatchSettings,
    },
    /// Take the first free seat in an existing room.
    Join { code: String, name: String },
    /// Play the given action; only accepted on your turn.
    Act(Action),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ServerMsg {
    /// You are seated. Build the board from `settings`; your seat is `slot`.
    Joined {
        code: String,
        slot: usize,
        settings: MatchSettings,
    },
    /// Seats changed. `None` is an open seat.
    Roster { names: Vec<Option<String>> },
    /// Every seat is taken; seat 0 moves first.
    Started,
    /// A move was accepted. Everyone applies it locally.
    Applied { slot: usize, action: Action },
    /// A seat was vacated during play; that player resigns.
    Resigned { slot: usize },
    /// Your last request was refused. Nothing changed.
    Rejected(String),
}

/// Serialize one message as a single JSON line.
pub fn write_msg<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let mut line = serde_json::to_string(msg).map_err(io::Error::other)?;
    line.push('\n');
    w.write_all(line.as_bytes())?;
    w.flush()
}

/// Parse one JSON line.
pub fn parse_msg<T: DeserializeOwned>(line: &str) -> Result<T, String> {
    serde_json::from_str(line).map_err(|e| format!("bad message: {e}"))
}

/// Normalize a host- or joiner-typed room code: trimmed, upper case,
/// 1..=`MAX_CODE_LEN` ASCII letters or digits.
pub fn clean_code(raw: &str) -> Result<String, RoomError> {
    let code = raw.trim().to_ascii_uppercase();
    let ok =
        (1..=MAX_CODE_LEN).contains(&code.len()) && code.chars().all(|c| c.is_ascii_alphanumeric());
    if ok {
        Ok(code)
    } else {
        Err(RoomError::BadCode)
    }
}

/// Trim, cap and default a player name so the roster stays readable.
pub fn clean_name(raw: &str, slot: usize) -> String {
    let name: String = raw.trim().chars().take(MAX_NAME_LEN).collect();
    if name.is_empty() {
        format!("Player {}", slot + 1)
    } else {
        name
    }
}

// ---------------------------------------------------------------------------
// Rooms (server side, pure)
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RoomError {
    BadCode,
    CodeTaken,
    NoSuchRoom,
    Full,
    AlreadyStarted,
    NotStarted,
    NotYourTurn,
    Rule(ActionError),
}

impl fmt::Display for RoomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RoomError::BadCode => write!(f, "room code must be 1-{MAX_CODE_LEN} letters or digits"),
            RoomError::CodeTaken => f.write_str("that room code is already in use"),
            RoomError::NoSuchRoom => f.write_str("no room with that code"),
            RoomError::Full => f.write_str("that room is full"),
            RoomError::AlreadyStarted => f.write_str("that game has already started"),
            RoomError::NotStarted => f.write_str("the game has not started yet"),
            RoomError::NotYourTurn => f.write_str("it is not your turn"),
            RoomError::Rule(e) => write!(f, "{e}"),
        }
    }
}

/// What happened when a seat was vacated.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Leave {
    /// Before the start: the seat is open again.
    Vacated,
    /// During play: the player resigned.
    Resigned,
}

#[derive(Debug)]
pub struct Room {
    pub settings: MatchSettings,
    pub seats: Vec<Option<String>>,
    pub game: Option<Game>,
}

impl Room {
    pub fn new(settings: MatchSettings) -> Room {
        Room {
            settings,
            seats: vec![None; settings.players],
            game: None,
        }
    }

    pub fn started(&self) -> bool {
        self.game.is_some()
    }

    pub fn is_full(&self) -> bool {
        self.seats.iter().all(Option::is_some)
    }

    /// Seat a player; returns their slot.
    pub fn join(&mut self, name: &str) -> Result<usize, RoomError> {
        if self.started() {
            return Err(RoomError::AlreadyStarted);
        }
        let slot = self
            .seats
            .iter()
            .position(Option::is_none)
            .ok_or(RoomError::Full)?;
        self.seats[slot] = Some(clean_name(name, slot));
        Ok(slot)
    }

    /// Build the board. Call once the room is full.
    pub fn start(&mut self) {
        let s = self.settings;
        let mut game = Game::new(s.width, s.height, s.players, s.seed);
        for (p, seat) in game.players.iter_mut().zip(&self.seats) {
            if let Some(name) = seat {
                p.name.clone_from(name);
            }
        }
        self.game = Some(game);
    }

    /// Apply an action on behalf of `slot`.
    pub fn act(&mut self, slot: usize, action: Action) -> Result<Outcome, RoomError> {
        let game = self.game.as_mut().ok_or(RoomError::NotStarted)?;
        if game.current != slot {
            return Err(RoomError::NotYourTurn);
        }
        game.apply(action).map_err(RoomError::Rule)
    }

    pub fn leave(&mut self, slot: usize) -> Leave {
        if let Some(game) = self.game.as_mut() {
            game.resign(slot);
            Leave::Resigned
        } else {
            self.seats[slot] = None;
            Leave::Vacated
        }
    }
}

/// All open rooms, keyed by their join code.
#[derive(Default)]
pub struct Lobby {
    rooms: HashMap<String, Room>,
}

impl Lobby {
    /// Open a room under the host's `code` with the creator in seat 0.
    /// Returns the normalized code and seat.
    pub fn create(
        &mut self,
        code: &str,
        settings: MatchSettings,
        name: &str,
    ) -> Result<(String, usize), RoomError> {
        let code = clean_code(code)?;
        if self.rooms.contains_key(&code) {
            return Err(RoomError::CodeTaken);
        }
        let mut room = Room::new(settings);
        let slot = room.join(name).expect("fresh room has a free seat");
        self.rooms.insert(code.clone(), room);
        Ok((code, slot))
    }

    /// Take a seat in the room under `code`. Returns the normalized code and seat.
    pub fn join(&mut self, code: &str, name: &str) -> Result<(String, usize), RoomError> {
        let code = clean_code(code).map_err(|_| RoomError::NoSuchRoom)?;
        let room = self.rooms.get_mut(&code).ok_or(RoomError::NoSuchRoom)?;
        let slot = room.join(name)?;
        Ok((code, slot))
    }

    pub fn room(&self, code: &str) -> Option<&Room> {
        self.rooms.get(code)
    }

    pub fn room_mut(&mut self, code: &str) -> Option<&mut Room> {
        self.rooms.get_mut(code)
    }

    pub fn remove(&mut self, code: &str) {
        self.rooms.remove(code);
    }
}

// ---------------------------------------------------------------------------
// Client socket (used by main.rs)
// ---------------------------------------------------------------------------

/// What the client's poll loop sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Msg(ServerMsg),
    /// The socket is gone. Reported once.
    Disconnected(String),
}

/// A connection to the server. Reading and writing happen on background
/// threads; the game loop only ever calls `send` and `poll`.
pub struct Client {
    stream: TcpStream,
    tx: Sender<ClientMsg>,
    /// `Receiver` is not `Sync`; the mutex makes `Client` usable as a Bevy resource.
    rx: Mutex<Receiver<Event>>,
    closed: bool,
}

impl Client {
    /// Connect with a short timeout so a wrong address fails fast.
    pub fn connect(addr: &str) -> io::Result<Client> {
        let target = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::other("address did not resolve"))?;
        let stream = TcpStream::connect_timeout(&target, CONNECT_TIMEOUT)?;
        stream.set_nodelay(true)?;

        let (tx, out_rx) = mpsc::channel::<ClientMsg>();
        let (in_tx, rx) = mpsc::channel::<Event>();

        let mut writer = stream.try_clone()?;
        thread::spawn(move || {
            for msg in out_rx {
                if write_msg(&mut writer, &msg).is_err() {
                    break;
                }
            }
        });

        let reader = BufReader::new(stream.try_clone()?);
        thread::spawn(move || {
            for line in reader.lines() {
                let event = match line {
                    Ok(line) => match parse_msg::<ServerMsg>(&line) {
                        Ok(msg) => Event::Msg(msg),
                        Err(e) => Event::Disconnected(e),
                    },
                    Err(e) => Event::Disconnected(e.to_string()),
                };
                let fatal = matches!(event, Event::Disconnected(_));
                if in_tx.send(event).is_err() || fatal {
                    return;
                }
            }
            let _ = in_tx.send(Event::Disconnected("server closed the connection".into()));
        });

        Ok(Client {
            stream,
            tx,
            rx: Mutex::new(rx),
            closed: false,
        })
    }

    /// Queue a message. Failures surface later as `Event::Disconnected`.
    pub fn send(&self, msg: ClientMsg) {
        let _ = self.tx.send(msg);
    }

    /// Next pending event, if any. Never blocks.
    pub fn poll(&mut self) -> Option<Event> {
        if self.closed {
            return None;
        }
        let polled = self
            .rx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .try_recv();
        match polled {
            Ok(Event::Disconnected(why)) => {
                self.closed = true;
                Some(Event::Disconnected(why))
            }
            Ok(ev) => Some(ev),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.closed = true;
                Some(Event::Disconnected("reader thread stopped".into()))
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Dir;

    fn settings(players: usize) -> MatchSettings {
        MatchSettings {
            width: 8,
            height: 8,
            players,
            seed: 11,
        }
    }

    #[test]
    fn messages_round_trip_as_json_lines() {
        let msgs = vec![
            ServerMsg::Joined {
                code: "ABCD".into(),
                slot: 1,
                settings: settings(3),
            },
            ServerMsg::Roster {
                names: vec![Some("ann".into()), None],
            },
            ServerMsg::Applied {
                slot: 0,
                action: Action::Move {
                    unit: 3,
                    dir: Dir::Left,
                },
            },
            ServerMsg::Rejected("nope".into()),
        ];
        let mut buf = Vec::new();
        for m in &msgs {
            write_msg(&mut buf, m).unwrap();
        }
        let text = String::from_utf8(buf).unwrap();
        let back: Vec<ServerMsg> = text.lines().map(|l| parse_msg(l).unwrap()).collect();
        assert_eq!(back, msgs);
        assert!(parse_msg::<ClientMsg>("{\"nonsense\":1}").is_err());
    }

    #[test]
    fn settings_validation_matches_menu_limits() {
        assert!(settings(2).validate().is_ok());
        assert!(settings(0).validate().is_err());
        assert!(settings(99).validate().is_err());
        let huge = MatchSettings {
            width: 1000,
            ..settings(2)
        };
        assert!(huge.validate().is_err());
    }

    #[test]
    fn room_fills_in_seat_order_then_rejects() {
        let mut room = Room::new(settings(2));
        assert_eq!(room.join("ann"), Ok(0));
        assert!(!room.is_full());
        assert_eq!(room.join("  "), Ok(1));
        assert_eq!(room.seats[1].as_deref(), Some("Player 2"));
        assert!(room.is_full());
        assert_eq!(room.join("late"), Err(RoomError::Full));
    }

    #[test]
    fn leaving_before_start_frees_the_seat() {
        let mut room = Room::new(settings(3));
        room.join("a").unwrap();
        room.join("b").unwrap();
        assert_eq!(room.leave(0), Leave::Vacated);
        assert_eq!(room.join("c"), Ok(0));
    }

    #[test]
    fn start_copies_names_and_enforces_turn_order() {
        let mut room = Room::new(settings(2));
        room.join("ann").unwrap();
        room.join("bob").unwrap();
        assert_eq!(room.act(0, Action::Pass), Err(RoomError::NotStarted));
        room.start();
        let g = room.game.as_ref().unwrap();
        assert_eq!(g.players[0].name, "ann");
        assert_eq!(g.players[1].name, "bob");
        assert_eq!(room.join("x"), Err(RoomError::AlreadyStarted));

        assert_eq!(room.act(1, Action::Pass), Err(RoomError::NotYourTurn));
        assert_eq!(room.act(0, Action::Pass), Ok(Outcome::Passed));
        assert_eq!(
            room.act(1, Action::Raise { unit: 999 }),
            Err(RoomError::Rule(ActionError::NoSuchUnit))
        );
        assert!(room
            .act(
                1,
                Action::Produce {
                    kind: crate::game::UnitKind::Tank,
                    at: None
                }
            )
            .is_ok());
    }

    #[test]
    fn leaving_during_play_resigns() {
        let mut room = Room::new(settings(2));
        room.join("ann").unwrap();
        room.join("bob").unwrap();
        room.start();
        assert_eq!(room.leave(1), Leave::Resigned);
        let g = room.game.as_ref().unwrap();
        assert!(!g.players[1].alive);
        assert!(g.result.is_some());
    }

    #[test]
    fn lobby_uses_host_chosen_code() {
        let mut lobby = Lobby::default();
        assert_eq!(
            lobby.create(" tank1 ", settings(2), "a"),
            Ok(("TANK1".to_string(), 0))
        );
        assert_eq!(
            lobby.create("Tank1", settings(2), "b"),
            Err(RoomError::CodeTaken)
        );
        assert_eq!(lobby.join("tank1", "c"), Ok(("TANK1".to_string(), 1)));
        assert_eq!(lobby.join("ZZZZ", "c"), Err(RoomError::NoSuchRoom));
        lobby.remove("TANK1");
        assert!(lobby.room("TANK1").is_none());
    }

    #[test]
    fn room_codes_are_validated() {
        assert_eq!(clean_code("ab12"), Ok("AB12".to_string()));
        assert_eq!(clean_code("   "), Err(RoomError::BadCode));
        assert_eq!(clean_code("a-b"), Err(RoomError::BadCode));
        assert_eq!(clean_code("ABCDEFGHI"), Err(RoomError::BadCode));
        let mut lobby = Lobby::default();
        assert_eq!(lobby.create("", settings(2), "a"), Err(RoomError::BadCode));
    }
}
