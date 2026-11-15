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
address and a name, and types a room code: the host picks it (1-8 letters or digits,
refused if already in use) and tells the others, who enter the same code to join. The match starts when every seat is taken. Only the current player
can act; the server validates every move and relays it to the others. A player who
disconnects resigns.

Shortcuts: `--connect HOST:PORT --name NAME --host CODE` or `--join CODE` connect immediately.

## Rules

- A map is generated (plains and impassable water) and every player gets a base with one tank.
- Players act in order. One of your tanks is preselected and blinks; click another or press Tab to cycle. One action per turn:
  - **Move** a selected tank one cell (arrows / WASD).
  - **Produce** a tank of strength 1 (P) or a settler (E) at a base you hold. Hover a held
    base to produce there; otherwise your home base is tried first, then the others.
  - **Found** a base with the selected settler (F): the settler becomes a base on its tile.
    Every base needs two free tiles to every other base, diagonals included.
  - **Raise** a selected tank's strength by 1 (R). Settlers cannot be raised.
  - Space passes. `M` returns to the menu.
- Moving onto an enemy tank fights: the stronger tank survives with the difference in strength.
  Equal strength destroys both.
- Settlers are peaceful, strength 0. A settler cannot enter a tile with another settler. A
  settler moving onto an enemy tank is destroyed; a tank moving onto a settler destroys it
  and keeps its strength. Settlers never capture bases.
- A tank ending a move on a base you do not hold captures it; its frame takes your colour. You produce
  at every base you hold (your home base first). A player whose home base is taken stays in
  the game and can retake it the same way.
- A player with no units left (tanks or settlers) is eliminated, even if they still hold bases.
  Founding a base with your last unit therefore knocks you out. Bases they
  held turn grey (neutral) and can be captured by anyone.
- Last player alive wins. If the last players knock each other out at once, the game is a draw.
- The end screen offers Play again (same settings, Enter) or Menu (M).

## Layout

- `src/game.rs` — pure rules, no Bevy. Unit tested (`cargo test`). Shared by server and clients.
- `src/net.rs` — wire protocol (JSON lines over TCP), room/lobby logic, client socket. Unit tested.
- `src/server.rs` — headless TCP server (`--serve`).
- `src/main.rs` — Bevy rendering, menu, input and HUD.
