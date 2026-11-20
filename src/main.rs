//! Bevy front end: menu to configure a match, then the board. Turns input into
//! `game::Action`s. Hot-seat (everyone shares one window) or online: each
//! player runs a client, one machine runs `beavy --serve ADDR`.

mod game;
mod net;
mod server;

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use game::{Action, Dir, Game, GameOver, Pos, Terrain, UnitKind, BUILD_TURNS};
use net::{Client, ClientMsg, Event, MatchSettings, ServerMsg};
use std::fmt::Write as _;

const CELL: f32 = 48.0;
const HUD_H: f32 = 115.0;
/// Empty space around the grid on every side.
const MARGIN: f32 = 24.0;
const MIN_WIN_W: f32 = 760.0;
const MENU_W: f32 = 760.0;
const MENU_H: f32 = 760.0;

const MIN_PLAYERS: usize = 2;
const MAX_PLAYERS: usize = 8;
const MIN_W: i32 = 6;
const MAX_W: i32 = 40;
const MIN_H: i32 = 6;
const MAX_H: i32 = 30;
/// Online: how long a dropped player's seat is held (seconds; 0 = resign at once).
const DEFAULT_RECONNECT_SECS: u32 = 60;
const MAX_RECONNECT_SECS: u32 = 600;
const RECONNECT_STEP: u32 = 15;

// ---------------------------------------------------------------------------
// Settings (prefilled from the command line, edited in the menu)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Everyone at one keyboard.
    Local,
    /// Open a room under a code of the host's choosing and wait for the others.
    Host,
    /// Enter a room code someone else is hosting.
    Join,
}

#[derive(Resource, Clone)]
struct Settings {
    players: usize,
    width: i32,
    height: i32,
    seed: u64,
    mode: Mode,
    /// Server address for Host/Join.
    addr: String,
    /// Name shown to other players online.
    name: String,
    /// Room code: chosen by the host, typed by joiners.
    code: String,
    /// Host only: seconds a dropped player may take to reconnect.
    reconnect_secs: u32,
    /// `--host` / `--join` on the command line: connect without waiting for Enter.
    autostart: bool,
    /// Hot-seat: who moves first. Each "Play again" passes it to the next seat.
    first_player: usize,
}

impl Settings {
    fn match_settings(&self) -> MatchSettings {
        MatchSettings {
            width: self.width,
            height: self.height,
            players: self.players,
            seed: self.seed,
            reconnect_secs: self.reconnect_secs,
        }
    }
}

fn now_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_secs())
}

/// Either run the headless server or start the client with these settings.
enum Launch {
    Client(Settings),
    Serve(String),
}

fn parse_args() -> Launch {
    let mut cfg = Settings {
        players: 3,
        width: 16,
        height: 12,
        seed: now_seed() % 100_000,
        mode: Mode::Local,
        addr: net::DEFAULT_ADDR.to_string(),
        name: String::new(),
        code: String::new(),
        reconnect_secs: DEFAULT_RECONNECT_SECS,
        autostart: false,
        first_player: 0,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let value = args.get(i + 1);
        match (args[i].as_str(), value) {
            ("--players" | "-p", Some(v)) => cfg.players = v.parse().expect("--players N"),
            ("--width" | "-w", Some(v)) => cfg.width = v.parse().expect("--width N"),
            ("--height" | "-h", Some(v)) => cfg.height = v.parse().expect("--height N"),
            ("--seed" | "-s", Some(v)) => cfg.seed = v.parse().expect("--seed N"),
            ("--reconnect", Some(v)) => {
                cfg.reconnect_secs = v.parse().expect("--reconnect SECS");
            }
            ("--serve", Some(v)) => return Launch::Serve(v.clone()),
            ("--connect" | "-c", Some(v)) => cfg.addr.clone_from(v),
            ("--name" | "-n", Some(v)) => cfg.name.clone_from(v),
            ("--host", Some(v)) => {
                cfg.mode = Mode::Host;
                cfg.code = v.to_ascii_uppercase();
                cfg.autostart = true;
            }
            ("--join" | "-j", Some(v)) => {
                cfg.mode = Mode::Join;
                cfg.code = v.to_ascii_uppercase();
                cfg.autostart = true;
            }
            ("--help", _) => {
                println!(
                    "usage: beavy [--players N] [--width N] [--height N] [--seed N]\n\
                     \x20      [--connect HOST:PORT] [--name NAME] [--host CODE | --join CODE]\n\
                     \x20      [--reconnect SECS]   (host: seat hold time, default 60, 0 = off)\n\
                     \x20      beavy --serve HOST:PORT   (run a game server)"
                );
                std::process::exit(0);
            }
            (other, _) => {
                eprintln!("unknown or incomplete argument: {other}");
                std::process::exit(2);
            }
        }
        i += 2;
    }
    cfg.clamp();
    Launch::Client(cfg)
}

impl Settings {
    fn clamp(&mut self) {
        self.width = self.width.clamp(MIN_W, MAX_W);
        self.height = self.height.clamp(MIN_H, MAX_H);
        // Game::new needs 5 cells per player.
        let by_area = usize::try_from(self.width * self.height / 5).unwrap_or(0);
        self.players = self.players.clamp(MIN_PLAYERS, MAX_PLAYERS.min(by_area));
        self.reconnect_secs = self.reconnect_secs.min(MAX_RECONNECT_SECS);
    }
}

// ---------------------------------------------------------------------------
// ECS state, resources and components
// ---------------------------------------------------------------------------

#[derive(States, Default, Debug, Clone, PartialEq, Eq, Hash)]
enum AppState {
    #[default]
    Menu,
    /// Online: connected, waiting for the room to fill.
    Waiting,
    Playing,
    /// One-frame hop used to rebuild the board with the same settings.
    Restart,
}

#[derive(Resource)]
struct GameRes(Game);

/// What the player's keys act on, and the one thing that blinks: a unit
/// (arrows, R, F) or a base that can still produce (P, E).
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
enum Selected {
    #[default]
    Nothing,
    Unit(u32),
    Base(Pos),
}

impl Selected {
    fn unit(self) -> Option<u32> {
        match self {
            Selected::Unit(id) => Some(id),
            _ => None,
        }
    }

    fn base(self) -> Option<Pos> {
        match self {
            Selected::Base(p) => Some(p),
            _ => None,
        }
    }
}

/// Live server connection plus what it told us. Present only when online.
#[derive(Resource)]
struct Net {
    client: Client,
    code: String,
    /// Our seat, known once the server says `Joined`.
    slot: Option<usize>,
    settings: MatchSettings,
    roster: Vec<Option<String>>,
    /// Set once the socket died; input is then disabled.
    lost: Option<String>,
}

/// One-line message under the menu (connection errors and the like).
#[derive(Resource, Default)]
struct MenuStatus(String);

/// Which text field receives typed characters.
#[derive(Resource, Default)]
struct Focus(Option<TextKind>);

/// Everything spawned for the waiting screen; despawned when leaving it.
#[derive(Component)]
struct WaitingEntity;

#[derive(Component)]
struct WaitingText;

#[derive(Component)]
struct StatusText;

/// Everything spawned for one match; despawned when leaving `Playing`.
#[derive(Component)]
struct BoardEntity;

/// Everything spawned for the menu; despawned when leaving `Menu`.
#[derive(Component)]
struct MenuEntity;

#[derive(Component)]
struct UnitMarker;

/// Hull sprite of a tank; pulses while that tank is selected.
#[derive(Component)]
struct Hull {
    unit: u32,
    owner: usize,
}

/// Base tile sprites; rebuilt whenever bases are founded or change hands.
#[derive(Component)]
struct BaseFrame;

/// The coloured outer frame of a base, which blinks while it can produce.
#[derive(Component)]
struct BaseRim {
    pos: Pos,
    holder: Option<usize>,
}

/// Shared mesh/material handles for tank parts.
#[derive(Resource)]
struct TankAssets {
    turret: Handle<Mesh>,
    turret_mats: Vec<Handle<ColorMaterial>>,
}

#[derive(Component)]
struct HoverMarker;

#[derive(Component)]
struct HudText;

/// Overlay shown once the match has a result.
#[derive(Component)]
struct EndScreen;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Players,
    Width,
    Height,
    Seed,
    Reconnect,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TextKind {
    Addr,
    Name,
    Code,
}

impl TextKind {
    fn max_len(self) -> usize {
        match self {
            TextKind::Addr => 48,
            TextKind::Name => net::MAX_NAME_LEN,
            TextKind::Code => net::MAX_CODE_LEN,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    Dec(Field),
    Inc(Field),
    SetMode(Mode),
    Edit(TextKind),
    Start,
    Replay,
    Menu,
}

#[derive(Component)]
struct MenuButton(MenuAction);

#[derive(Component)]
struct ValueText(Field);

#[derive(Component)]
struct FieldText(TextKind);

const PLAYER_COLORS: [(f32, f32, f32); 8] = [
    (0.20, 0.55, 0.95), // blue
    (0.90, 0.25, 0.25), // red
    (0.25, 0.75, 0.35), // green
    (0.95, 0.75, 0.15), // yellow
    (0.70, 0.35, 0.85), // purple
    (0.95, 0.50, 0.15), // orange
    (0.20, 0.80, 0.80), // teal
    (0.90, 0.45, 0.70), // pink
];

const PLAYER_COLOR_NAMES: [&str; 8] = [
    "blue", "red", "green", "yellow", "purple", "orange", "teal", "pink",
];

const BG: Color = Color::srgb(0.08, 0.09, 0.11);
const BTN: Color = Color::srgb(0.20, 0.22, 0.27);
const BTN_HOVER: Color = Color::srgb(0.30, 0.33, 0.40);
const BTN_PRESS: Color = Color::srgb(0.16, 0.45, 0.80);
const START: Color = Color::srgb(0.18, 0.55, 0.30);
const START_HOVER: Color = Color::srgb(0.24, 0.68, 0.38);
const FIELD: Color = Color::srgb(0.12, 0.13, 0.16);
const FIELD_FOCUS: Color = Color::srgb(0.18, 0.22, 0.30);

fn player_color(i: usize) -> Color {
    let (r, g, b) = PLAYER_COLORS[i % PLAYER_COLORS.len()];
    Color::srgb(r, g, b)
}

/// Grid coordinate to f32. Boards are at most 40 cells wide, so the value
/// always fits an i16, which converts to f32 exactly.
fn grid_f32(v: i32) -> f32 {
    f32::from(i16::try_from(v).expect("grid coordinate fits i16"))
}

fn cell_center(p: Pos, z: f32) -> Vec3 {
    Vec3::new(
        (grid_f32(p.x) + 0.5) * CELL,
        (grid_f32(p.y) + 0.5) * CELL,
        z,
    )
}

// Truncation is the point: `floor` already ran and the result is bounds-checked
// by every caller, so an out-of-range float only yields an off-map cell.
#[allow(clippy::cast_possible_truncation)]
fn world_to_cell(w: Vec2) -> Pos {
    Pos::new((w.x / CELL).floor() as i32, (w.y / CELL).floor() as i32)
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

fn main() {
    let settings = match parse_args() {
        Launch::Serve(addr) => {
            if let Err(e) = server::run(&addr) {
                eprintln!("server failed: {e}");
                std::process::exit(1);
            }
            return;
        }
        Launch::Client(settings) => settings,
    };
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Beavy Tanks".into(),
                resolution: (MENU_W, MENU_H).into(),
                resizable: false,
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(BG))
        .insert_resource(settings)
        .init_resource::<Selected>()
        .init_resource::<MenuStatus>()
        .init_resource::<Focus>()
        .init_state::<AppState>()
        .add_systems(Startup, (spawn_camera, load_tank_assets))
        // Menu
        .add_systems(
            OnEnter(AppState::Menu),
            (|mut s: ResMut<Settings>| s.first_player = 0, build_menu),
        )
        .add_systems(
            Update,
            (menu_autostart, menu_keys, menu_typing, sync_menu_values)
                .chain()
                .run_if(in_state(AppState::Menu)),
        )
        .add_systems(Update, handle_buttons)
        .add_systems(
            OnEnter(AppState::Restart),
            |mut settings: ResMut<Settings>, mut next: ResMut<NextState<AppState>>| {
                // Same map, the next seat opens.
                settings.first_player = (settings.first_player + 1) % settings.players;
                next.set(AppState::Playing);
            },
        )
        .add_systems(OnExit(AppState::Menu), despawn_all::<MenuEntity>)
        // Waiting room (online)
        .add_systems(OnEnter(AppState::Waiting), build_waiting)
        .add_systems(
            Update,
            (waiting_keys, net_poll_waiting, sync_waiting_text)
                .chain()
                .run_if(in_state(AppState::Waiting)),
        )
        .add_systems(OnExit(AppState::Waiting), despawn_all::<WaitingEntity>)
        // Match
        .add_systems(OnEnter(AppState::Playing), build_board)
        .add_systems(
            Update,
            (
                net_poll_playing,
                reconnect_keys,
                handle_input,
                sync_units,
                sync_bases,
                sync_hover,
                sync_hud,
                pulse_selected,
                pulse_bases,
                show_end_screen,
            )
                .chain()
                .run_if(in_state(AppState::Playing)),
        )
        .add_systems(
            OnExit(AppState::Playing),
            (despawn_all::<BoardEntity>, teardown_match),
        )
        .run();
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn(Camera2d);
}

fn load_tank_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let turret = meshes.add(Circle::new(CELL * 0.17));
    let turret_mats = (0..PLAYER_COLORS.len())
        .map(|i| materials.add(player_color(i).darker(0.12)))
        .collect();
    commands.insert_resource(TankAssets {
        turret,
        turret_mats,
    });
}

fn despawn_all<T: Component>(mut commands: Commands, q: Query<Entity, With<T>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

// ---------------------------------------------------------------------------
// Menu
// ---------------------------------------------------------------------------

fn font(size: f32) -> TextFont {
    TextFont {
        font_size: size,
        ..default()
    }
}

fn build_menu(
    mut commands: Commands,
    settings: Res<Settings>,
    status: Res<MenuStatus>,
    mut focus: ResMut<Focus>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut cameras: Query<&mut Transform, With<Camera2d>>,
) {
    // Back in the menu means any online session is over.
    commands.remove_resource::<Net>();
    focus.0 = None;
    if let Ok(mut w) = windows.single_mut() {
        w.resolution.set(MENU_W, MENU_H);
    }
    if let Ok(mut t) = cameras.single_mut() {
        t.translation = Vec3::ZERO;
    }

    commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(12.0),
                ..default()
            },
            MenuEntity,
        ))
        .with_children(|root| {
            root.spawn((
                Text::new("BEAVY TANKS"),
                font(40.0),
                TextColor(Color::srgb(0.95, 0.85, 0.30)),
                Node {
                    margin: UiRect::bottom(Val::Px(6.0)),
                    ..default()
                },
            ));

            for (label, field) in [
                ("Players", Field::Players),
                ("Board width", Field::Width),
                ("Board height", Field::Height),
                ("Map seed", Field::Seed),
                ("Reconnect time", Field::Reconnect),
            ] {
                menu_row(root, label, |row| spawn_stepper(row, field));
            }

            menu_row(root, "Mode", |row| {
                for (label, mode) in [
                    ("Local", Mode::Local),
                    ("Host", Mode::Host),
                    ("Join", Mode::Join),
                ] {
                    let color = mode_color(mode, settings.mode);
                    spawn_button(row, label, MenuAction::SetMode(mode), color, 100.0, &font(20.0));
                }
            });
            for (label, kind) in [
                ("Server", TextKind::Addr),
                ("Your name", TextKind::Name),
                ("Room code", TextKind::Code),
            ] {
                menu_row(root, label, |row| spawn_text_field(row, kind));
            }

            root.spawn(Node {
                margin: UiRect::top(Val::Px(10.0)),
                ..default()
            })
            .with_children(|row| {
                spawn_button(
                    row,
                    start_label(settings.mode),
                    MenuAction::Start,
                    START,
                    260.0,
                    &font(26.0),
                );
            });

            root.spawn((
                Text::new(status.0.clone()),
                font(16.0),
                TextColor(Color::srgb(0.95, 0.55, 0.45)),
                StatusText,
            ));
            root.spawn((
                Text::new(
                    "Arrows: board size | +/-: players | S: reroll seed | click a text field to type\n\
                     Online: one machine runs `beavy --serve 0.0.0.0:7777`; host picks a room code, others join with it",
                ),
                font(14.0),
                TextColor(Color::srgb(0.55, 0.55, 0.6)),
                TextLayout::new_with_justify(JustifyText::Center),
            ));
        });
}

/// A labelled row of controls.
/// `-  value  +` controls for a numeric setting.
fn spawn_stepper(row: &mut ChildSpawnerCommands, field: Field) {
    spawn_button(row, "-", MenuAction::Dec(field), BTN, 44.0, &font(24.0));
    row.spawn((
        Text::new(""),
        font(24.0),
        TextColor(Color::WHITE),
        TextLayout::new_with_justify(JustifyText::Center),
        Node {
            width: Val::Px(110.0),
            ..default()
        },
        ValueText(field),
    ));
    spawn_button(row, "+", MenuAction::Inc(field), BTN, 44.0, &font(24.0));
}

fn menu_row(
    root: &mut ChildSpawnerCommands,
    label: &str,
    controls: impl FnOnce(&mut ChildSpawnerCommands),
) {
    root.spawn(Node {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        column_gap: Val::Px(12.0),
        ..default()
    })
    .with_children(|row| {
        row.spawn((
            Text::new(label),
            font(22.0),
            TextColor(Color::srgb(0.9, 0.9, 0.9)),
            Node {
                width: Val::Px(170.0),
                ..default()
            },
        ));
        controls(row);
    });
}

/// A click-to-focus text box. Its content is filled by `sync_menu_values`.
fn spawn_text_field(row: &mut ChildSpawnerCommands, kind: TextKind) {
    row.spawn((
        Button,
        Node {
            width: Val::Px(310.0),
            height: Val::Px(40.0),
            align_items: AlignItems::Center,
            padding: UiRect::horizontal(Val::Px(10.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BorderColor(Color::srgb(0.45, 0.48, 0.55)),
        BorderRadius::all(Val::Px(6.0)),
        BackgroundColor(FIELD),
        MenuButton(MenuAction::Edit(kind)),
    ))
    .with_children(|b| {
        b.spawn((
            Text::new(""),
            font(20.0),
            TextColor(Color::WHITE),
            FieldText(kind),
        ));
    });
}

fn spawn_button(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    action: MenuAction,
    color: Color,
    width: f32,
    font: &TextFont,
) {
    parent
        .spawn((
            Button,
            Node {
                width: Val::Px(width),
                height: Val::Px(44.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BorderColor(Color::srgb(0.45, 0.48, 0.55)),
            BorderRadius::all(Val::Px(6.0)),
            BackgroundColor(color),
            MenuButton(action),
        ))
        .with_children(|b| {
            b.spawn((Text::new(label), font.clone(), TextColor(Color::WHITE)));
        });
}

fn start_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Local => "START  (Enter)",
        Mode::Host => "HOST GAME  (Enter)",
        Mode::Join => "JOIN GAME  (Enter)",
    }
}

fn mode_color(button: Mode, active: Mode) -> Color {
    if button == active {
        BTN_PRESS
    } else {
        BTN
    }
}

/// Resting colour of a button, given the current settings and focus.
fn idle_color(action: MenuAction, settings: &Settings, focus: &Focus) -> Color {
    match action {
        MenuAction::Start | MenuAction::Replay => START,
        MenuAction::SetMode(m) => mode_color(m, settings.mode),
        MenuAction::Edit(k) if focus.0 == Some(k) => FIELD_FOCUS,
        MenuAction::Edit(_) => FIELD,
        _ => BTN,
    }
}

/// Step a setting. `delta` is -1, 0 or +1; 0 on the seed means "reroll".
fn adjust(settings: &mut Settings, field: Field, delta: i32) {
    const SEED_MOD: u64 = 100_000;
    match field {
        Field::Players => {
            settings.players = if delta >= 0 {
                settings.players + 1
            } else {
                settings.players.saturating_sub(1)
            };
        }
        Field::Width => settings.width += delta,
        Field::Height => settings.height += delta,
        Field::Seed if delta == 0 => {
            settings.seed = now_seed().wrapping_mul(2_654_435_761) % SEED_MOD;
        }
        Field::Seed => {
            let step = if delta > 0 { 1 } else { SEED_MOD - 1 };
            settings.seed = (settings.seed + step) % SEED_MOD;
        }
        Field::Reconnect => {
            settings.reconnect_secs = if delta >= 0 {
                settings.reconnect_secs + RECONNECT_STEP
            } else {
                settings.reconnect_secs.saturating_sub(RECONNECT_STEP)
            };
        }
    }
    settings.clamp();
}

/// Start whatever the menu is set up for. Online modes connect first and go
/// to the waiting room; a failed connection stays in the menu with a message.
fn begin_match(
    settings: &Settings,
    commands: &mut Commands,
    next: &mut NextState<AppState>,
    status: &mut MenuStatus,
) {
    if settings.mode == Mode::Local {
        next.set(AppState::Playing);
        return;
    }
    if settings.code.trim().is_empty() {
        status.0 = match settings.mode {
            Mode::Host => "Choose a room code for the others to join with.",
            _ => "Enter the room code to join.",
        }
        .into();
        return;
    }
    let client = match Client::connect(settings.addr.trim()) {
        Ok(c) => c,
        Err(e) => {
            status.0 = format!("Could not connect to {}: {e}", settings.addr.trim());
            return;
        }
    };
    let name = settings.name.clone();
    client.send(match settings.mode {
        Mode::Host => ClientMsg::Create {
            code: settings.code.trim().to_ascii_uppercase(),
            name,
            settings: settings.match_settings(),
        },
        _ => ClientMsg::Join {
            code: settings.code.trim().to_ascii_uppercase(),
            name,
        },
    });
    commands.insert_resource(Net {
        client,
        code: String::new(),
        slot: None,
        settings: settings.match_settings(),
        roster: Vec::new(),
        lost: None,
    });
    status.0.clear();
    next.set(AppState::Waiting);
}

/// Shared handler for every UI button (menu and end screen).
fn handle_buttons(
    mut commands: Commands,
    mut settings: ResMut<Settings>,
    mut focus: ResMut<Focus>,
    mut status: ResMut<MenuStatus>,
    mut next: ResMut<NextState<AppState>>,
    mut q: Query<
        (&Interaction, &MenuButton, &mut BackgroundColor),
        (Changed<Interaction>, With<Button>),
    >,
) {
    for (interaction, button, mut bg) in &mut q {
        match *interaction {
            Interaction::Pressed => {
                *bg = BackgroundColor(BTN_PRESS);
                // Clicking anything but a text field drops keyboard focus.
                if !matches!(button.0, MenuAction::Edit(_)) && focus.0.is_some() {
                    focus.0 = None;
                }
                match button.0 {
                    MenuAction::Dec(f) => adjust(&mut settings, f, -1),
                    MenuAction::Inc(f) => adjust(&mut settings, f, 1),
                    MenuAction::SetMode(m) => settings.mode = m,
                    MenuAction::Edit(k) => focus.0 = Some(k),
                    MenuAction::Start => {
                        begin_match(&settings, &mut commands, &mut next, &mut status);
                    }
                    MenuAction::Replay => next.set(AppState::Restart),
                    MenuAction::Menu => next.set(AppState::Menu),
                }
            }
            Interaction::Hovered => {
                *bg = BackgroundColor(match button.0 {
                    MenuAction::Start | MenuAction::Replay => START_HOVER,
                    MenuAction::Edit(_) => FIELD_FOCUS,
                    _ => BTN_HOVER,
                });
            }
            Interaction::None => *bg = BackgroundColor(idle_color(button.0, &settings, &focus)),
        }
    }
}

/// Honour `--host` / `--join` once, on the first menu frame.
fn menu_autostart(
    mut commands: Commands,
    mut settings: ResMut<Settings>,
    mut status: ResMut<MenuStatus>,
    mut next: ResMut<NextState<AppState>>,
) {
    if settings.autostart {
        settings.autostart = false;
        begin_match(&settings, &mut commands, &mut next, &mut status);
    }
}

fn menu_keys(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut settings: ResMut<Settings>,
    mut focus: ResMut<Focus>,
    mut status: ResMut<MenuStatus>,
    mut next: ResMut<NextState<AppState>>,
) {
    if keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter]) {
        if focus.0.is_some() {
            focus.0 = None;
        } else {
            begin_match(&settings, &mut commands, &mut next, &mut status);
        }
        return;
    }
    if keys.just_pressed(KeyCode::Escape) && focus.0.is_some() {
        focus.0 = None;
        return;
    }
    if focus.0.is_some() {
        // Typing goes to the field; see `menu_typing`.
        return;
    }
    let mut change: Option<(Field, i32)> = None;
    if keys.just_pressed(KeyCode::ArrowRight) {
        change = Some((Field::Width, 1));
    } else if keys.just_pressed(KeyCode::ArrowLeft) {
        change = Some((Field::Width, -1));
    } else if keys.just_pressed(KeyCode::ArrowUp) {
        change = Some((Field::Height, 1));
    } else if keys.just_pressed(KeyCode::ArrowDown) {
        change = Some((Field::Height, -1));
    } else if keys.any_just_pressed([KeyCode::Equal, KeyCode::NumpadAdd]) {
        change = Some((Field::Players, 1));
    } else if keys.any_just_pressed([KeyCode::Minus, KeyCode::NumpadSubtract]) {
        change = Some((Field::Players, -1));
    } else if keys.just_pressed(KeyCode::KeyS) {
        change = Some((Field::Seed, 0));
    }
    if let Some((field, delta)) = change {
        adjust(&mut settings, field, delta);
    }
}

/// Append typed characters to the focused text field.
fn menu_typing(
    mut events: EventReader<KeyboardInput>,
    focus: Res<Focus>,
    mut settings: ResMut<Settings>,
) {
    let Some(kind) = focus.0 else {
        events.clear();
        return;
    };
    for ev in events.read() {
        if !ev.state.is_pressed() {
            continue;
        }
        let max = kind.max_len();
        let field = match kind {
            TextKind::Addr => &mut settings.addr,
            TextKind::Name => &mut settings.name,
            TextKind::Code => &mut settings.code,
        };
        match &ev.logical_key {
            Key::Backspace => {
                field.pop();
            }
            Key::Space if kind == TextKind::Name => push_capped(field, " ", max),
            Key::Character(text) => {
                let text = if kind == TextKind::Code {
                    text.to_ascii_uppercase()
                } else {
                    text.to_string()
                };
                let clean: String = text
                    .chars()
                    .filter(|c| !c.is_control())
                    .filter(|c| kind != TextKind::Code || c.is_ascii_alphanumeric())
                    .collect();
                push_capped(field, &clean, max);
            }
            _ => {}
        }
    }
}

fn push_capped(field: &mut String, text: &str, max: usize) {
    for c in text.chars() {
        if field.chars().count() >= max {
            break;
        }
        field.push(c);
    }
}

fn sync_menu_values(
    settings: Res<Settings>,
    focus: Res<Focus>,
    status: Res<MenuStatus>,
    mut values: Query<(&ValueText, &mut Text), (Without<FieldText>, Without<StatusText>)>,
    mut fields: Query<(&FieldText, &mut Text), (Without<ValueText>, Without<StatusText>)>,
    mut starts: Query<(&MenuButton, &Children)>,
    mut labels: Query<&mut Text, (Without<ValueText>, Without<FieldText>, Without<StatusText>)>,
    mut status_text: Query<&mut Text, With<StatusText>>,
    mut buttons: Query<(&MenuButton, &Interaction, &mut BackgroundColor)>,
) {
    if status.is_changed() {
        if let Ok(mut t) = status_text.single_mut() {
            t.0.clone_from(&status.0);
        }
    }
    if !(settings.is_changed() || focus.is_changed()) {
        return;
    }
    for (field, mut text) in &mut values {
        text.0 = match field.0 {
            Field::Players => settings.players.to_string(),
            Field::Width => settings.width.to_string(),
            Field::Height => settings.height.to_string(),
            Field::Seed => settings.seed.to_string(),
            Field::Reconnect => match settings.reconnect_secs {
                0 => "off".to_string(),
                secs => format!("{secs}s"),
            },
        };
    }
    for (field, mut text) in &mut fields {
        let value = match field.0 {
            TextKind::Addr => &settings.addr,
            TextKind::Name => &settings.name,
            TextKind::Code => &settings.code,
        };
        let cursor = if focus.0 == Some(field.0) { "_" } else { "" };
        text.0 = format!("{value}{cursor}");
    }
    for (button, children) in &mut starts {
        if button.0 == MenuAction::Start {
            for child in children {
                if let Ok(mut t) = labels.get_mut(*child) {
                    t.0 = start_label(settings.mode).to_string();
                }
            }
        }
    }
    for (button, interaction, mut bg) in &mut buttons {
        if *interaction == Interaction::None {
            *bg = BackgroundColor(idle_color(button.0, &settings, &focus));
        }
    }
}

// ---------------------------------------------------------------------------
// Waiting room (online)
// ---------------------------------------------------------------------------

fn build_waiting(mut commands: Commands) {
    commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(14.0),
                ..default()
            },
            WaitingEntity,
        ))
        .with_children(|root| {
            root.spawn((
                Text::new("Connecting..."),
                font(26.0),
                TextColor(Color::WHITE),
                TextLayout::new_with_justify(JustifyText::Center),
                WaitingText,
            ));
            root.spawn((
                Text::new("Esc: back to menu"),
                font(14.0),
                TextColor(Color::srgb(0.55, 0.55, 0.6)),
            ));
        });
}

fn waiting_keys(keys: Res<ButtonInput<KeyCode>>, mut next: ResMut<NextState<AppState>>) {
    if keys.just_pressed(KeyCode::Escape) {
        next.set(AppState::Menu);
    }
}

/// Handle server messages until the room is full, then start the match.
fn net_poll_waiting(
    net: Option<ResMut<Net>>,
    mut status: ResMut<MenuStatus>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    let Some(mut net) = net else {
        next_state.set(AppState::Menu);
        return;
    };
    while let Some(event) = net.client.poll() {
        match event {
            Event::Msg(ServerMsg::Joined {
                code,
                slot,
                settings,
            }) => {
                net.code = code;
                net.slot = Some(slot);
                net.settings = settings;
            }
            Event::Msg(ServerMsg::Roster { names }) => net.roster = names,
            Event::Msg(ServerMsg::Started) => {
                next_state.set(AppState::Playing);
                return;
            }
            Event::Msg(ServerMsg::Rejected(why)) => {
                status.0 = format!("Server refused: {why}");
                next_state.set(AppState::Menu);
                return;
            }
            Event::Msg(_) => {}
            Event::Disconnected(why) => {
                status.0 = format!("Connection lost: {why}");
                next_state.set(AppState::Menu);
                return;
            }
        }
    }
}

fn sync_waiting_text(net: Option<Res<Net>>, mut q: Query<&mut Text, With<WaitingText>>) {
    let Some(net) = net else { return };
    if !net.is_changed() {
        return;
    }
    let Ok(mut text) = q.single_mut() else {
        return;
    };
    let mut s = String::new();
    if net.code.is_empty() {
        s.push_str("Connecting...");
    } else {
        let taken = net.roster.iter().flatten().count();
        let _ = writeln!(s, "Room code: {}", net.code);
        let _ = writeln!(
            s,
            "{}x{} board, seed {}",
            net.settings.width, net.settings.height, net.settings.seed
        );
        let _ = writeln!(
            s,
            "\nWaiting for players ({taken}/{})...\n",
            net.settings.players
        );
        for (i, seat) in net.roster.iter().enumerate() {
            let you = if net.slot == Some(i) { " (you)" } else { "" };
            match seat {
                Some(name) => {
                    let _ = writeln!(s, "{}: {name}{you}", color_name(i));
                }
                None => {
                    let _ = writeln!(s, "{}: open", color_name(i));
                }
            }
        }
    }
    text.0 = s;
}

// ---------------------------------------------------------------------------
// Board setup / teardown
// ---------------------------------------------------------------------------

fn build_board(
    mut commands: Commands,
    settings: Res<Settings>,
    net: Option<Res<Net>>,
    mut selected: ResMut<Selected>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut cameras: Query<&mut Transform, With<Camera2d>>,
) {
    // Online, the server dictates the board; every client builds the same one.
    let ms = net
        .as_ref()
        .map_or_else(|| settings.match_settings(), |n| n.settings);
    // Online, seat 0 always opens; hot-seat rematches rotate the opener.
    let first = if net.is_some() {
        0
    } else {
        settings.first_player
    };
    let mut game = Game::with_first_player(ms.width, ms.height, ms.players, ms.seed, first);
    if let Some(net) = &net {
        for (p, seat) in game.players.iter_mut().zip(&net.roster) {
            if let Some(name) = seat {
                p.name.clone_from(name);
            }
        }
    }
    *selected = first_ready(&game, acting_player(&game, net.as_deref()));

    let grid_w = grid_f32(game.width) * CELL;
    let grid_h = grid_f32(game.height) * CELL;
    let win_w = (grid_w + 2.0 * MARGIN).max(MIN_WIN_W);
    let win_h = grid_h + 2.0 * MARGIN + HUD_H;

    if let Ok(mut w) = windows.single_mut() {
        w.resolution.set(win_w, win_h);
    }
    // Camera centred on the grid, shifted up by half the HUD so the band sits
    // above the grid with equal margins around it.
    if let Ok(mut t) = cameras.single_mut() {
        t.translation = Vec3::new(grid_w / 2.0, grid_h / 2.0 + HUD_H / 2.0, 0.0);
    }

    // Frame behind the grid.
    commands.spawn((
        Sprite::from_color(
            Color::srgb(0.14, 0.15, 0.18),
            Vec2::new(grid_w + MARGIN, grid_h + MARGIN),
        ),
        Transform::from_xyz(grid_w / 2.0, grid_h / 2.0, -0.5),
        BoardEntity,
    ));

    // Terrain tiles.
    for y in 0..game.height {
        for x in 0..game.width {
            let p = Pos::new(x, y);
            let color = match game.terrain_at(p) {
                Terrain::Plain => {
                    if (x + y) % 2 == 0 {
                        Color::srgb(0.36, 0.52, 0.30)
                    } else {
                        Color::srgb(0.33, 0.48, 0.28)
                    }
                }
                Terrain::Water => Color::srgb(0.16, 0.32, 0.62),
            };
            commands.spawn((
                Sprite::from_color(color, Vec2::splat(CELL - 2.0)),
                Transform::from_translation(cell_center(p, 0.0)),
                BoardEntity,
            ));
        }
    }

    // Hover and selection highlights.
    commands.spawn((
        Sprite::from_color(Color::srgba(1.0, 1.0, 1.0, 0.18), Vec2::splat(CELL)),
        Transform::from_xyz(0.0, 0.0, 0.5),
        Visibility::Hidden,
        HoverMarker,
        BoardEntity,
    ));

    spawn_hud(&mut commands);

    commands.insert_resource(GameRes(game));
}

/// HUD: fixed band at the top, clipped so text never spills onto the grid.
fn spawn_hud(commands: &mut Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Px(HUD_H),
                padding: UiRect::axes(Val::Px(12.0), Val::Px(10.0)),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(BG),
            BoardEntity,
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new(""),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(Color::srgb(0.92, 0.92, 0.92)),
                Node {
                    width: Val::Percent(100.0),
                    ..default()
                },
                HudText,
            ));
        });
}

fn teardown_match(mut commands: Commands) {
    commands.remove_resource::<GameRes>();
}

// ---------------------------------------------------------------------------
// Match input
// ---------------------------------------------------------------------------

fn cursor_cell(
    windows: &Query<&Window, With<PrimaryWindow>>,
    cameras: &Query<(&Camera, &GlobalTransform)>,
) -> Option<Pos> {
    let window = windows.single().ok()?;
    let (camera, cam_tf) = cameras.single().ok()?;
    let cursor = window.cursor_position()?;
    let world = camera.viewport_to_world_2d(cam_tf, cursor).ok()?;
    Some(world_to_cell(world))
}

/// Whose tanks this window controls: the player to act when hot-seat, our
/// seat when online.
fn acting_player(g: &Game, net: Option<&Net>) -> usize {
    net.and_then(|n| n.slot).unwrap_or(g.current)
}

/// What `owner` can still use this turn: ready units first, then bases that
/// can produce. Once nothing is ready, all their units, so Tab still inspects.
fn selectable(g: &Game, owner: usize) -> Vec<Selected> {
    let own = || g.units.iter().filter(move |u| u.owner == owner);
    let ready: Vec<Selected> = own()
        .filter(|u| !u.acted)
        .map(|u| Selected::Unit(u.id))
        .chain(
            g.held_bases(owner)
                .filter(|&p| g.can_produce_at(owner, p))
                .map(Selected::Base),
        )
        .collect();
    if ready.is_empty() {
        own().map(|u| Selected::Unit(u.id)).collect()
    } else {
        ready
    }
}

/// Whether `sel` is something `owner` can still use this turn.
fn is_ready(g: &Game, owner: usize, sel: Selected) -> bool {
    match sel {
        Selected::Nothing => false,
        Selected::Unit(id) => g.unit(id).is_some_and(|u| u.owner == owner && !u.acted),
        Selected::Base(p) => g.can_produce_at(owner, p),
    }
}

/// First ready unit or base of `owner`, used as the default selection.
/// `Nothing` once everything is spent, so nothing spent looks selected.
fn first_ready(g: &Game, owner: usize) -> Selected {
    selectable(g, owner)
        .into_iter()
        .find(|&s| is_ready(g, owner, s))
        .unwrap_or_default()
}

/// Next selectable unit or base of `owner` after `current`, wrapping around.
fn next_selectable(g: &Game, owner: usize, current: Selected) -> Selected {
    let all = selectable(g, owner);
    let idx = all
        .iter()
        .position(|&x| x == current)
        .map_or(0, |i| (i + 1) % all.len().max(1));
    all.get(idx).copied().unwrap_or_default()
}

/// Move the selection on to the next ready unit or base of `owner` once the
/// selected one is gone, spent, or not theirs.
fn advance_selection(g: &Game, owner: usize, selected: &mut Selected) {
    if !is_ready(g, owner, *selected) {
        let fresh = first_ready(g, owner);
        if *selected != fresh {
            *selected = fresh;
        }
    }
}

/// What a click on `cell` selects: one of our units, else one of our bases,
/// else nothing.
fn clicked(g: &Game, me: usize, cell: Option<Pos>) -> Selected {
    let Some(p) = cell.filter(|&p| g.in_bounds(p)) else {
        return Selected::Nothing;
    };
    if let Some(u) = g.unit_at(p).filter(|u| u.owner == me) {
        Selected::Unit(u.id)
    } else if g.base_at(p).is_some_and(|i| g.bases[i].holder == Some(me)) {
        Selected::Base(p)
    } else {
        Selected::Nothing
    }
}

fn handle_input(
    mut game: ResMut<GameRes>,
    net: Option<Res<Net>>,
    mut selected: ResMut<Selected>,
    mut next_state: ResMut<NextState<AppState>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
) {
    if keys.just_pressed(KeyCode::KeyM) {
        next_state.set(AppState::Menu);
        return;
    }
    let online = net.is_some();
    if game.0.result.is_some() {
        if !online && keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter]) {
            next_state.set(AppState::Restart);
        }
        return;
    }
    if net.as_ref().is_some_and(|n| n.lost.is_some()) {
        return;
    }
    let me = acting_player(&game.0, net.as_deref());

    if mouse.just_pressed(MouseButton::Left) {
        let picked = clicked(&game.0, me, cursor_cell(&windows, &cameras));
        if *selected != picked {
            *selected = picked;
        }
    }

    if keys.just_pressed(KeyCode::Escape) && *selected != Selected::Nothing {
        *selected = Selected::Nothing;
    }
    if keys.just_pressed(KeyCode::Tab) {
        *selected = next_selectable(&game.0, me, *selected);
    }

    // Keyboard: pick exactly one action.
    let dir = if keys.any_just_pressed([KeyCode::ArrowUp, KeyCode::KeyW]) {
        Some(Dir::Up)
    } else if keys.any_just_pressed([KeyCode::ArrowDown, KeyCode::KeyS]) {
        Some(Dir::Down)
    } else if keys.any_just_pressed([KeyCode::ArrowLeft, KeyCode::KeyA]) {
        Some(Dir::Left)
    } else if keys.any_just_pressed([KeyCode::ArrowRight, KeyCode::KeyD]) {
        Some(Dir::Right)
    } else {
        None
    };

    let action = if let Some(dir) = dir {
        if let Some(unit) = selected.unit() {
            Some(Action::Move { unit, dir })
        } else {
            game.0
                .log
                .push("Select one of your units first (click it).".into());
            None
        }
    } else if keys.any_just_pressed([KeyCode::KeyP, KeyCode::KeyE]) {
        let kind = if keys.just_pressed(KeyCode::KeyP) {
            UnitKind::Tank
        } else {
            UnitKind::Settler
        };
        // Produce at the selected base, else the held base under the cursor,
        // else let the rules pick.
        let g: &Game = &game.0;
        let at = selected.base().or_else(|| {
            cursor_cell(&windows, &cameras)
                .filter(|&p| g.base_at(p).is_some_and(|i| g.bases[i].holder == Some(me)))
        });
        Some(Action::Produce { kind, at })
    } else if keys.just_pressed(KeyCode::KeyF) {
        if let Some(unit) = selected.unit() {
            Some(Action::Found { unit })
        } else {
            game.0.log.push("Select a settler to found a base.".into());
            None
        }
    } else if keys.just_pressed(KeyCode::KeyR) {
        if let Some(unit) = selected.unit() {
            Some(Action::Raise { unit })
        } else {
            game.0
                .log
                .push("Select a tank to raise its strength.".into());
            None
        }
    } else if keys.just_pressed(KeyCode::Space) {
        Some(Action::Pass)
    } else {
        None
    };

    let Some(action) = action else { return };
    if let Some(net) = net {
        // Online: the server decides. The move comes back as `Applied`.
        if game.0.current == me {
            net.client.send(ClientMsg::Act(action));
        } else {
            game.0.log.push("Wait for your turn.".into());
        }
        return;
    }
    match game.0.apply(action) {
        Ok(_) => advance_selection(&game.0, game.0.current, &mut selected),
        Err(e) => game.0.log.push(format!("Not allowed: {e}.")),
    }
}

/// Online, after the connection dropped (or desynced): Enter joins the same
/// room again. The server hands back our held seat and replays the match.
fn reconnect_keys(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    net: Option<Res<Net>>,
    mut game: ResMut<GameRes>,
    mut settings: ResMut<Settings>,
    mut status: ResMut<MenuStatus>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    let Some(net) = net else { return };
    if net.lost.is_none() || !keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter]) {
        return;
    }
    settings.mode = Mode::Join;
    settings.code.clone_from(&net.code);
    if let Some(name) = net.slot.and_then(|i| net.roster.get(i).cloned().flatten()) {
        settings.name = name;
    }
    status.0.clear();
    begin_match(&settings, &mut commands, &mut next_state, &mut status);
    if !status.0.is_empty() {
        game.0
            .log
            .push(format!("{} Press Enter to retry.", status.0));
    }
}

/// Online: apply what the server broadcasts.
fn net_poll_playing(
    net: Option<ResMut<Net>>,
    mut game: ResMut<GameRes>,
    mut selected: ResMut<Selected>,
) {
    let Some(mut net) = net else { return };
    while let Some(event) = net.client.poll() {
        match event {
            Event::Msg(ServerMsg::Applied { slot, action }) => {
                if game.0.current != slot {
                    game.0.log.push(
                        "Out of sync with the server. Press Enter to reload the match, M for the menu."
                            .into(),
                    );
                    net.lost = Some("desync".into());
                    return;
                }
                if let Err(e) = game.0.apply(action) {
                    game.0.log.push(format!("Server move failed locally: {e}."));
                }
            }
            Event::Msg(ServerMsg::Resigned { slot }) => game.0.resign(slot),
            Event::Msg(ServerMsg::Rejected(why)) => {
                game.0.log.push(format!("Not allowed: {why}."));
            }
            Event::Msg(ServerMsg::Roster { names }) => net.roster = names,
            Event::Msg(ServerMsg::Away { slot, secs }) => {
                let name = game.0.players[slot].name.clone();
                game.0.log.push(format!(
                    "{name} lost connection; their seat is held for {secs}s."
                ));
            }
            Event::Msg(ServerMsg::Back { slot }) => {
                let name = game.0.players[slot].name.clone();
                game.0.log.push(format!("{name} is back."));
            }
            Event::Msg(_) => {}
            Event::Disconnected(why) => {
                game.0.log.push(format!(
                    "Connection lost: {why}. Press Enter to reconnect, M for the menu."
                ));
                net.lost = Some(why);
                return;
            }
        }
    }
    // Keep a valid selection: our next ready unit if the old one is gone or spent.
    let me = acting_player(&game.0, Some(&net));
    advance_selection(&game.0, me, &mut selected);
}

// ---------------------------------------------------------------------------
// Match rendering
// ---------------------------------------------------------------------------

/// Rebuild unit sprites whenever the game state changes. Unit counts are tiny,
/// so despawn-and-respawn is simpler than diffing.
fn sync_units(
    mut commands: Commands,
    game: Res<GameRes>,
    assets: Res<TankAssets>,
    existing: Query<Entity, With<UnitMarker>>,
) {
    if !game.is_changed() {
        return;
    }
    for e in &existing {
        commands.entity(e).despawn();
    }
    for u in &game.0.units {
        match u.kind {
            UnitKind::Tank => spawn_tank(&mut commands, &assets, u),
            UnitKind::Settler => spawn_settler(&mut commands, u),
        }
    }
}

fn facing_angle(dir: Dir) -> f32 {
    match dir {
        Dir::Right => 0.0,
        Dir::Up => std::f32::consts::FRAC_PI_2,
        Dir::Left => std::f32::consts::PI,
        Dir::Down => -std::f32::consts::FRAC_PI_2,
    }
}

/// Top-down tank: two tracks, hull, round turret, barrel. The body rotates to
/// face the last move; the strength badge stays upright.
fn spawn_tank(commands: &mut Commands, assets: &TankAssets, u: &game::Unit) {
    let color = player_color(u.owner);
    let dark = Color::srgb(0.13, 0.13, 0.14);
    let track = Color::srgb(0.22, 0.22, 0.24);
    let turret_mat = assets.turret_mats[u.owner % assets.turret_mats.len()].clone();

    commands
        .spawn((
            Transform::from_translation(cell_center(u.pos, 1.0)),
            Visibility::default(),
            UnitMarker,
            BoardEntity,
        ))
        .with_children(|root| {
            // Rotating body.
            root.spawn((
                Transform::from_rotation(Quat::from_rotation_z(facing_angle(u.facing))),
                Visibility::default(),
            ))
            .with_children(|body| {
                for side in [-1.0, 1.0] {
                    body.spawn((
                        Sprite::from_color(track, Vec2::new(CELL * 0.80, CELL * 0.18)),
                        Transform::from_xyz(0.0, side * CELL * 0.26, 0.0),
                    ));
                    // Track tread marks.
                    for k in -3..=3 {
                        body.spawn((
                            Sprite::from_color(dark, Vec2::new(CELL * 0.04, CELL * 0.18)),
                            Transform::from_xyz(
                                grid_f32(k) * CELL * 0.11,
                                side * CELL * 0.26,
                                0.01,
                            ),
                        ));
                    }
                }
                body.spawn((
                    Sprite::from_color(color, Vec2::new(CELL * 0.70, CELL * 0.40)),
                    Transform::from_xyz(0.0, 0.0, 0.1),
                    Hull {
                        unit: u.id,
                        owner: u.owner,
                    },
                ));
                // Barrel and muzzle brake, from the turret centre forwards.
                body.spawn((
                    Sprite::from_color(dark, Vec2::new(CELL * 0.40, CELL * 0.08)),
                    Transform::from_xyz(CELL * 0.22, 0.0, 0.15),
                ));
                body.spawn((
                    Sprite::from_color(dark, Vec2::new(CELL * 0.07, CELL * 0.14)),
                    Transform::from_xyz(CELL * 0.40, 0.0, 0.16),
                ));
                body.spawn((
                    Mesh2d(assets.turret.clone()),
                    MeshMaterial2d(turret_mat),
                    Transform::from_xyz(0.0, 0.0, 0.2),
                ));
            });
            // Strength badge, always upright.
            root.spawn((
                Text2d::new(u.strength.to_string()),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                Transform::from_xyz(0.0, 0.0, 0.3),
            ));
        });
}

/// Top-down settler wagon: canvas cover in the owner's colour on a dark bed
/// with four wheels. No strength badge; settlers are always 0.
fn spawn_settler(commands: &mut Commands, u: &game::Unit) {
    let dark = Color::srgb(0.13, 0.13, 0.14);
    let bed = Color::srgb(0.45, 0.32, 0.18);
    commands
        .spawn((
            Transform::from_translation(cell_center(u.pos, 1.0)),
            Visibility::default(),
            UnitMarker,
            BoardEntity,
        ))
        .with_children(|root| {
            for (x, y) in [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)] {
                root.spawn((
                    Sprite::from_color(dark, Vec2::new(CELL * 0.16, CELL * 0.10)),
                    Transform::from_xyz(x * CELL * 0.22, y * CELL * 0.24, 0.0),
                ));
            }
            root.spawn((
                Sprite::from_color(bed, Vec2::new(CELL * 0.62, CELL * 0.42)),
                Transform::from_xyz(0.0, 0.0, 0.05),
            ));
            root.spawn((
                Sprite::from_color(player_color(u.owner), Vec2::new(CELL * 0.50, CELL * 0.32)),
                Transform::from_xyz(0.0, 0.0, 0.1),
                Hull {
                    unit: u.id,
                    owner: u.owner,
                },
            ));
            // Cover hoops.
            for k in -1..=1 {
                root.spawn((
                    Sprite::from_color(Color::WHITE, Vec2::new(CELL * 0.04, CELL * 0.32)),
                    Transform::from_xyz(grid_f32(k) * CELL * 0.14, 0.0, 0.15),
                ));
            }
        });
}

/// Pulse the hull of the selected tank so the active player sees which tank
/// their keys will drive. Online, a window off-turn keeps its selection but
/// stops pulsing, so only the player to act has a blinking tank. Units of the
/// player to act that are spent this turn are dimmed.
fn pulse_selected(
    game: Res<GameRes>,
    selected: Res<Selected>,
    time: Res<Time>,
    mut hulls: Query<(&Hull, &mut Sprite)>,
) {
    let phase = (time.elapsed_secs() * 6.0).sin() * 0.5 + 0.5;
    for (hull, mut sprite) in &mut hulls {
        let base = player_color(hull.owner);
        let to_act = hull.owner == game.0.current;
        let spent = to_act && game.0.unit(hull.unit).is_some_and(|u| u.acted);
        sprite.color = if spent {
            base.mix(&Color::BLACK, 0.55)
        } else if Some(hull.unit) == selected.unit() && to_act {
            base.mix(&Color::WHITE, 0.7 * phase)
        } else {
            base
        };
    }
}

/// Blink the frame of the selected base while it can still produce, like the
/// selected unit's hull. Online, only on our turn.
fn pulse_bases(
    game: Res<GameRes>,
    net: Option<Res<Net>>,
    selected: Res<Selected>,
    time: Res<Time>,
    mut rims: Query<(&BaseRim, &mut Sprite)>,
) {
    let g = &game.0;
    let our_turn = g.result.is_none() && acting_player(g, net.as_deref()) == g.current;
    let phase = (time.elapsed_secs() * 4.0).sin() * 0.5 + 0.5;
    for (rim, mut sprite) in &mut rims {
        let base = base_color(rim.holder);
        let ready = our_turn
            && *selected == Selected::Base(rim.pos)
            && g.can_produce_at(g.current, rim.pos);
        sprite.color = if ready {
            base.mix(&Color::WHITE, 0.7 * phase)
        } else {
            base
        };
    }
}

/// Frame colour for a base: its holder's colour, grey when nobody holds it.
fn base_color(holder: Option<usize>) -> Color {
    holder.map_or(Color::srgb(0.6, 0.6, 0.6), player_color)
}

/// Rebuild base tiles whenever the game state changes: a frame in the
/// holder's colour around a dark tile. Despawn-and-respawn like units.
fn sync_bases(
    mut commands: Commands,
    game: Res<GameRes>,
    existing: Query<Entity, With<BaseFrame>>,
) {
    if !game.is_changed() {
        return;
    }
    for e in &existing {
        commands.entity(e).despawn();
    }
    for base in &game.0.bases {
        commands.spawn((
            Sprite::from_color(base_color(base.holder), Vec2::splat(CELL - 2.0)),
            Transform::from_translation(cell_center(base.pos, 0.2)),
            BaseFrame,
            BaseRim {
                pos: base.pos,
                holder: base.holder,
            },
            BoardEntity,
        ));
        commands.spawn((
            Sprite::from_color(Color::srgb(0.22, 0.22, 0.22), Vec2::splat(CELL - 14.0)),
            Transform::from_translation(cell_center(base.pos, 0.3)),
            BaseFrame,
            BoardEntity,
        ));
        // What is being built and turns left, e.g. "T2", in the corner above units.
        if let Some(build) = base.build {
            let letter = match build.kind {
                UnitKind::Tank => 'T',
                UnitKind::Settler => 'S',
            };
            let left = build.ready.saturating_sub(game.0.turn);
            commands.spawn((
                Text2d::new(format!("{letter}{left}")),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(Color::srgb(0.95, 0.85, 0.30)),
                Transform::from_translation(
                    cell_center(base.pos, 5.0) + Vec3::new(-CELL * 0.3, -CELL * 0.33, 0.0),
                ),
                BaseFrame,
                BoardEntity,
            ));
        }
    }
}

fn sync_hover(
    game: Res<GameRes>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    mut hover_q: Query<(&mut Transform, &mut Visibility), With<HoverMarker>>,
) {
    let g = &game.0;
    if let Ok((mut tf, mut vis)) = hover_q.single_mut() {
        match cursor_cell(&windows, &cameras).filter(|&p| g.in_bounds(p)) {
            Some(p) => {
                tf.translation = cell_center(p, 0.5);
                *vis = Visibility::Visible;
            }
            None => *vis = Visibility::Hidden,
        }
    }
}

fn color_name(i: usize) -> &'static str {
    PLAYER_COLOR_NAMES[i % PLAYER_COLOR_NAMES.len()]
}

/// Title, title colour and one status line per player for the end screen.
fn end_summary(g: &Game, result: GameOver) -> (String, Color, Vec<String>) {
    let (title, title_color) = match result {
        GameOver::Winner(w) => (
            format!("{} ({}) wins!", g.players[w].name, color_name(w)),
            player_color(w).lighter(0.15),
        ),
        GameOver::Tie => (
            "Draw - nobody is left".to_string(),
            Color::srgb(0.85, 0.85, 0.85),
        ),
    };
    let summary = g
        .players
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let status = match result {
                GameOver::Winner(w) if w == i => "winner",
                _ if p.alive => "alive",
                _ => "eliminated",
            };
            let power: u32 = g
                .units
                .iter()
                .filter(|u| u.owner == i)
                .map(|u| u.strength)
                .sum();
            format!(
                "{} ({}): {}, power {}",
                p.name,
                color_name(i),
                status,
                power
            )
        })
        .collect();
    (title, title_color, summary)
}

/// Spawn the end-of-match overlay once the rules report a result.
fn show_end_screen(
    mut commands: Commands,
    game: Res<GameRes>,
    settings: Res<Settings>,
    net: Option<Res<Net>>,
    mut selected: ResMut<Selected>,
    existing: Query<(), With<EndScreen>>,
) {
    let g = &game.0;
    let Some(result) = g.result else {
        return;
    };
    if !existing.is_empty() {
        return;
    }
    if *selected != Selected::Nothing {
        *selected = Selected::Nothing;
    }

    let (title, title_color, summary) = end_summary(g, result);

    let font = |size: f32| TextFont {
        font_size: size,
        ..default()
    };
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(12.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.85)),
            GlobalZIndex(10),
            EndScreen,
            BoardEntity,
        ))
        .with_children(|root| {
            root.spawn((
                Text::new("GAME OVER"),
                font(20.0),
                TextColor(Color::srgb(0.7, 0.7, 0.7)),
            ));
            root.spawn((Text::new(title), font(40.0), TextColor(title_color)));
            root.spawn((
                Text::new(format!("{} turns played", g.turn)),
                font(16.0),
                TextColor(Color::srgb(0.8, 0.8, 0.8)),
            ));
            for line in summary {
                root.spawn((
                    Text::new(line),
                    font(16.0),
                    TextColor(Color::srgb(0.9, 0.9, 0.9)),
                ));
            }
            root.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(16.0),
                margin: UiRect::top(Val::Px(20.0)),
                ..default()
            })
            .with_children(|row| {
                // Online rematches would need a new room; go via the menu.
                if net.is_none() {
                    let opener = (settings.first_player + 1) % g.players.len();
                    spawn_button(
                        row,
                        &format!("Play again, {} starts (Enter)", color_name(opener)),
                        MenuAction::Replay,
                        START,
                        330.0,
                        &font(20.0),
                    );
                }
                spawn_button(row, "Menu (M)", MenuAction::Menu, BTN, 160.0, &font(20.0));
            });
        });
}

/// The HUD line describing the selected unit or base, if any.
fn selection_line(g: &Game, selected: Selected) -> Option<String> {
    if let Some(p) = selected.base() {
        let line = match g.base_at(p).and_then(|i| g.bases[i].build) {
            Some(b) => format!(
                "Selected: base at ({}, {}) | building a {}, rolls out on turn {}\n",
                p.x,
                p.y,
                b.kind.name(),
                b.ready
            ),
            None => format!(
                "Selected: base at ({}, {}) | P builds a tank, E a settler ({BUILD_TURNS} turns)\n",
                p.x, p.y
            ),
        };
        return Some(line);
    }
    let u = selected.unit().and_then(|id| g.unit(id))?;
    Some(format!(
        "Selected: {} #{} at ({}, {}) strength {}{}\n",
        u.kind.name(),
        u.id,
        u.pos.x,
        u.pos.y,
        u.strength,
        if u.acted { " (done this turn)" } else { "" }
    ))
}

fn sync_hud(
    game: Res<GameRes>,
    net: Option<Res<Net>>,
    selected: Res<Selected>,
    mut hud: Query<&mut Text, With<HudText>>,
) {
    if !(game.is_changed() || selected.is_changed()) {
        return;
    }
    let Ok(mut text) = hud.single_mut() else {
        return;
    };
    let g = &game.0;

    let mut s = String::new();
    if g.result.is_some() {
        let _ = writeln!(s, "GAME OVER after {} turns", g.turn);
    } else {
        let cur = g.current_player();
        let _ = write!(
            s,
            "Turn {}  |  {} ({}) to act",
            g.turn,
            cur.name,
            color_name(g.current)
        );
        let units = g
            .units
            .iter()
            .filter(|u| u.owner == g.current && !u.acted)
            .count();
        let bases = g
            .bases
            .iter()
            .filter(|b| b.holder == Some(g.current) && !b.acted)
            .count();
        let _ = write!(s, "  |  Ready: {units} units, {bases} bases");
        if let Some(net) = &net {
            let me = acting_player(g, Some(net));
            let _ = write!(s, "  |  Room {}  |  You: {}", net.code, color_name(me));
            if g.current != me {
                s.push_str("  |  waiting for them...");
            }
        }
        s.push('\n');
    }

    let roster: Vec<String> = g
        .players
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if !p.alive {
                return format!("{}: out", p.name);
            }
            let own = || g.units.iter().filter(move |u| u.owner == i);
            let tanks = own().filter(|u| u.kind == UnitKind::Tank).count();
            let settlers = own().filter(|u| u.kind == UnitKind::Settler).count();
            let power: u32 = own().map(|u| u.strength).sum();
            let bases = g.held_bases(i).count();
            format!("{}: {tanks}t {settlers}s {bases}b {power}p", p.name)
        })
        .collect();
    s.push_str(&roster.join("  |  "));
    s.push('\n');

    // Our turn and nothing left to move: say so instead of leaving no blinking unit.
    let our_turn = acting_player(g, net.as_deref()) == g.current;
    let all_done =
        g.result.is_none() && our_turn && !g.units.iter().any(|u| u.owner == g.current && !u.acted);
    if let Some(line) = selection_line(g, *selected) {
        s.push_str(&line);
    } else if g.result.is_none() && !all_done {
        s.push_str("Selected: none (click one of your units or bases, or press Tab)\n");
    }
    if all_done {
        s.push_str("All your units have acted: produce at a base (P / E) or press Space to end the turn.\n");
    }

    let _ = writeln!(
        s,
        "Blinking = selected unit or base (click or Tab) | Arrows move | P tank | E settler (at selected base, {BUILD_TURNS} turns) | R raise (at own base) | F found base | Space end turn | M menu"
    );
    if let Some(last) = g.log.last() {
        s.push_str(last);
    }

    text.0 = s;
}
