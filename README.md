# Beavy Tanks

Turn-based 2D tank strategy built with [Bevy](https://bevyengine.org). Play hot-seat on one
machine or online through a small game server.

## Run

```sh
cargo run --release
```

A menu lets you pick players (2-8), board width (6-40), height (6-30) and map seed,
with +/- buttons or keyboard (arrows = width/height, +/- = players, S = reroll seed,
Enter = start). Press `M` during a match to return to the menu.

CLI flags prefill the menu: `--players N --width N --height N --seed N`.

## Play online

One machine runs the server (no window, no GPU needed):

```sh
cargo run --release -- --serve 0.0.0.0:7777
```

Each player runs the client, picks **Host** or **Join** in the menu, types the server
address and a name, and either opens a room (a 4-letter code is shown while waiting)
or enters a code. The match starts when every seat is taken. Only the current player
can act; the server validates every move and relays it to the others. A player who
disconnects resigns.

Shortcuts: `--connect HOST:PORT --name NAME --host` or `--join CODE` connect immediately.

## Rules

- A map is generated (plains and impassable water) and every player gets a base with one tank.
- Players act in order. One of your tanks is preselected and blinks; click another or press Tab to cycle. One action per turn:
  - **Move** a selected tank one cell (arrows / WASD).
  - **Produce** a new tank of strength 1 at your base (P).
  - **Raise** a selected tank's strength by 1 (R).
  - Space passes. `M` returns to the menu.
- Moving onto an enemy tank fights: the stronger tank survives with the difference in strength.
  Equal strength destroys both.
- Standing a tank on an enemy base captures it: that player is eliminated and their tanks removed.
- A player whose last tank is destroyed is eliminated.
- Last player alive wins. If the last players knock each other out at once, the game is a draw.
- The end screen offers Play again (same settings, Enter) or Menu (M).

## Layout

- `src/game.rs` — pure rules, no Bevy. Unit tested (`cargo test`). Shared by server and clients.
- `src/net.rs` — wire protocol (JSON lines over TCP), room/lobby logic, client socket. Unit tested.
- `src/server.rs` — headless TCP server (`--serve`).
- `src/main.rs` — Bevy rendering, menu, input and HUD.
