//! Headless game server: `beavy --serve 0.0.0.0:7777`. One thread per
//! connection, one `Lobby` behind a mutex. All rules stay in `game.rs`; all
//! room logic stays in `net.rs`. This file only moves bytes.

use crate::net::{self, ClientMsg, Leave, Lobby, Room, RoomError, ServerMsg};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

/// Where a connected player sits.
#[derive(Clone)]
struct Seat {
    code: String,
    slot: usize,
}

struct Server {
    lobby: Lobby,
    /// Outgoing channel per seat, by room code. `None` is an empty seat.
    conns: HashMap<String, Vec<Option<Sender<ServerMsg>>>>,
}

type Shared = Arc<Mutex<Server>>;

/// Run forever, accepting connections on `addr`.
pub fn run(addr: &str) -> io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    println!("beavy server listening on {addr}");
    let shared: Shared = Arc::new(Mutex::new(Server {
        lobby: Lobby::default(),
        conns: HashMap::new(),
    }));
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let shared = Arc::clone(&shared);
                thread::spawn(move || handle_connection(stream, &shared));
            }
            Err(e) => eprintln!("accept failed: {e}"),
        }
    }
    Ok(())
}

fn lock(shared: &Shared) -> std::sync::MutexGuard<'_, Server> {
    // A panic while holding the lock only poisons it; the state is still usable.
    shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn handle_connection(stream: TcpStream, shared: &Shared) {
    let who = stream
        .peer_addr()
        .map_or_else(|_| "?".to_string(), |a| a.to_string());
    let _ = stream.set_nodelay(true);
    let (tx, rx) = mpsc::channel::<ServerMsg>();

    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    thread::spawn(move || {
        for msg in rx {
            if net::write_msg(&mut writer, &msg).is_err() {
                break;
            }
        }
    });

    let mut seat: Option<Seat> = None;
    for line in BufReader::new(&stream).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match net::parse_msg::<ClientMsg>(&line) {
            Ok(msg) => lock(shared).handle(&mut seat, msg, &tx),
            Err(e) => {
                let _ = tx.send(ServerMsg::Rejected(e));
            }
        }
    }
    if let Some(seat) = seat {
        lock(shared).leave(&seat);
    }
    println!("{who} disconnected");
}

impl Server {
    fn handle(&mut self, seat: &mut Option<Seat>, msg: ClientMsg, tx: &Sender<ServerMsg>) {
        let result = match msg {
            ClientMsg::Create {
                code,
                name,
                settings,
            } => {
                if seat.is_some() {
                    Err("you are already in a room".to_string())
                } else {
                    settings.validate().and_then(|()| {
                        let (code, slot) = self
                            .lobby
                            .create(&code, settings, &name)
                            .map_err(|e| e.to_string())?;
                        self.conns
                            .insert(code.clone(), vec![None; settings.players]);
                        self.sit(seat, Seat { code, slot }, tx);
                        Ok(())
                    })
                }
            }
            ClientMsg::Join { code, name } => {
                if seat.is_some() {
                    Err("you are already in a room".to_string())
                } else {
                    self.lobby
                        .join(&code, &name)
                        .map_err(|e| e.to_string())
                        .map(|(code, slot)| self.sit(seat, Seat { code, slot }, tx))
                }
            }
            ClientMsg::Act(action) => seat
                .as_ref()
                .ok_or_else(|| "join a room first".to_string())
                .and_then(|s| self.act(s, action)),
        };
        if let Err(e) = result {
            let _ = tx.send(ServerMsg::Rejected(e));
        }
    }

    /// Register a freshly seated player and tell the room.
    fn sit(&mut self, seat: &mut Option<Seat>, s: Seat, tx: &Sender<ServerMsg>) {
        let Some(room) = self.lobby.room(&s.code) else {
            return;
        };
        let settings = room.settings;
        if let Some(slots) = self.conns.get_mut(&s.code) {
            slots[s.slot] = Some(tx.clone());
        }
        let _ = tx.send(ServerMsg::Joined {
            code: s.code.clone(),
            slot: s.slot,
            settings,
        });
        self.broadcast_roster(&s.code);
        let full = self.lobby.room(&s.code).is_some_and(Room::is_full);
        if full {
            if let Some(room) = self.lobby.room_mut(&s.code) {
                room.start();
            }
            self.broadcast(&s.code, &ServerMsg::Started);
            println!("room {} started", s.code);
        }
        *seat = Some(s);
    }

    fn act(&mut self, s: &Seat, action: crate::game::Action) -> Result<(), String> {
        let room = self
            .lobby
            .room_mut(&s.code)
            .ok_or_else(|| RoomError::NoSuchRoom.to_string())?;
        room.act(s.slot, action).map_err(|e| e.to_string())?;
        self.broadcast(
            &s.code,
            &ServerMsg::Applied {
                slot: s.slot,
                action,
            },
        );
        Ok(())
    }

    fn leave(&mut self, s: &Seat) {
        let Some(room) = self.lobby.room_mut(&s.code) else {
            return;
        };
        let how = room.leave(s.slot);
        if let Some(slots) = self.conns.get_mut(&s.code) {
            slots[s.slot] = None;
        }
        match how {
            Leave::Vacated => self.broadcast_roster(&s.code),
            Leave::Resigned => self.broadcast(&s.code, &ServerMsg::Resigned { slot: s.slot }),
        }
        let nobody_left = self
            .conns
            .get(&s.code)
            .is_some_and(|slots| slots.iter().all(Option::is_none));
        if nobody_left {
            self.lobby.remove(&s.code);
            self.conns.remove(&s.code);
            println!("room {} closed", s.code);
        }
    }

    fn broadcast_roster(&self, code: &str) {
        if let Some(room) = self.lobby.room(code) {
            let names = room.seats.clone();
            self.broadcast(code, &ServerMsg::Roster { names });
        }
    }

    fn broadcast(&self, code: &str, msg: &ServerMsg) {
        if let Some(slots) = self.conns.get(code) {
            for tx in slots.iter().flatten() {
                let _ = tx.send(msg.clone());
            }
        }
    }
}
