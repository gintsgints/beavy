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
can act; the server validates every move and relays it to the others.

If a player drops mid-game, their seat is held for the host's **Reconnect time** (menu, or
`--reconnect SECS`; default 60 s, 0 = off, max 600 s). Play waits meanwhile. To come back,
press Enter in the game window, or Join the same room code again with the same name; the
server replays the match so far. If the time runs out, the player resigns.

Shortcuts: `--connect HOST:PORT --name NAME --host CODE` or `--join CODE` connect immediately.

## Rules

- A map is generated (plains and impassable water) and every player gets a base with one tank.
- Players act in order. Each turn every unit acts once and every idle base you hold can start a build.
  Exactly one thing blinks: the selected unit, or the selected base once your units are done.
  Click a unit or base, or press Tab to cycle through what can still act:
  - **Move** a selected tank one cell (arrows / WASD).
  - **Produce** a tank of strength 1 (T) or a settler (E) at a base you hold: the selected
    base, else a held base under the cursor, else your home base first, then the others.
    Building takes 3 turns: ordered on turn 5, the unit rolls out at the start of your turn 8,
    on or next to the base, and can act at once. A base builds one unit at a time (its tile
    shows e.g. `T2`: a tank, 2 turns left). If every cell there is taken, the unit waits for
    room. A base that is captured loses its build.
  - **Found** a base with the selected settler (F): the settler becomes a base on its tile.
    Every base needs two free tiles to every other base, diagonals included.
  - **Raise** a selected tank's strength by 1 (R) while it stands on or next to (not diagonally)
    a base you hold. Settlers cannot be raised.
  - Space ends your turn. It also ends by itself once none of your units or bases can do
    anything, and a player who cannot do anything at all is skipped. Spent units are dimmed;
    bases you just captured or founded start building from your next turn. `M` returns to
    the menu.
- Moving onto an enemy tank fights: the stronger tank survives with the difference in strength.
  Equal strength destroys both.
- Settlers are peaceful, strength 0. A settler cannot enter a tile with another settler. A
  settler moving onto an enemy tank is destroyed; a tank moving onto a settler destroys it
  and keeps its strength. Settlers never capture bases.
- A tank ending a move on a base you do not hold captures it; its frame takes your colour. You produce
  at every base you hold (your home base first). A player whose home base is taken stays in
  the game and can retake it the same way.
- A player is eliminated only when they have no units (tanks or settlers) and hold no base.
  With a base left they stay in and can produce again; losing that base while unitless ends them.
- A player who resigns (or does not reconnect in time) is out; bases they held turn grey
  (neutral) and anyone can capture them.
- Last player alive wins. If the last players knock each other out at once, the game is a draw.
- The end screen offers Play again (Enter) or Menu (M). Play again keeps the same map and settings
  and hands the first move to the next player; starting from the menu, Player 1 opens again.

## Layout

- `src/game.rs` — pure rules, no Bevy. Unit tested (`cargo test`). Shared by server and clients.
- `src/net.rs` — wire protocol (JSON lines over TCP), room/lobby logic, client socket. Unit tested.
- `src/server.rs` — headless TCP server (`--serve`).
- `src/main.rs` — Bevy rendering, menu, input and HUD.
