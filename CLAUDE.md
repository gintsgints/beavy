# Beavy Tanks — project rules

Hot-seat turn-based tank strategy on Bevy 0.16. Rust 2021.

## Layout

- `src/game.rs` — pure rules, no Bevy imports. All gameplay logic and its unit tests live here.
- `src/net.rs` — online protocol (`ClientMsg`/`ServerMsg`, JSON lines), `Room`/`Lobby` seating rules, `Client` socket wrapper. No Bevy. Unit tested.
- `src/server.rs` — headless TCP server behind `--serve ADDR`. Only moves bytes; no rules here.
- `src/main.rs` — Bevy only: states (Menu / Waiting / Playing), rendering, input, HUD.
- Keep that split. New rules go in `game.rs` with a test; room rules go in `net.rs` with a test; `main.rs` only translates input to `game::Action` and draws state.
- Online play is lockstep: server owns the `Game`, broadcasts accepted `Action`s only; clients rebuild the board from the shared seed. Never serialize board state.

## Mandatory after every code change

Run all three, in order, and fix everything before reporting done:

```sh
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

- `cargo fmt` output is the source of truth for formatting. Never hand-format against it.
- Clippy runs with both `clippy::all` and `clippy::pedantic` at `deny` (configured in `Cargo.toml` under `[lints.clippy]`), so even a plain `cargo clippy` fails on any finding. Pedantic lints, including numeric cast lints, must be fixed, not allowed.
- Fix lints by changing code. Add a `#[allow(...)]` only with a one-line comment saying why, and prefer a project-wide allow in `Cargo.toml` over scattered attributes.
- Do not weaken lint levels in `Cargo.toml` to make a change pass.
- A change is not done if any of the three commands fails or prints warnings.

## Conventions

- Deterministic map generation: same seed, same map. Keep `game::Rng` as the only randomness source in rules.
- Bevy systems: tag per-screen entities (`MenuEntity`, `BoardEntity`) so state exits can despawn them.
- Limits (players 2–8, board 6–40 × 6–30) live in constants at the top of `main.rs`; change them there only.
- No new dependencies without a reason stated in the commit message.
