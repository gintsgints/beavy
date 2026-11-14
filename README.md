# Beavy Tanks

Hot-seat, turn-based 2D tank strategy built with [Bevy](https://bevyengine.org).

## Run

```sh
cargo run --release
```

A menu lets you pick players (2-8), board width (6-40), height (6-30) and map seed,
with +/- buttons or keyboard (arrows = width/height, +/- = players, S = reroll seed,
Enter = start). Press `M` during a match to return to the menu.

CLI flags prefill the menu: `--players N --width N --height N --seed N`.

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

- `src/game.rs` — pure rules, no Bevy. Unit tested (`cargo test`). Reusable by a future network server.
- `src/main.rs` — Bevy rendering, input and HUD.
