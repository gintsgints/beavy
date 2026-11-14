//! Bevy front end: menu to configure a match, then the board. Turns input into
//! `game::Action`s. Hot-seat multiplayer: all players share one window.

mod game;

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use game::{Action, Dir, Game, GameOver, Pos, Terrain};
use std::fmt::Write as _;

const CELL: f32 = 48.0;
const HUD_H: f32 = 115.0;
/// Empty space around the grid on every side.
const MARGIN: f32 = 24.0;
const MIN_WIN_W: f32 = 760.0;
const MENU_W: f32 = 760.0;
const MENU_H: f32 = 520.0;

const MIN_PLAYERS: usize = 2;
const MAX_PLAYERS: usize = 8;
const MIN_W: i32 = 6;
const MAX_W: i32 = 40;
const MIN_H: i32 = 6;
const MAX_H: i32 = 30;

// ---------------------------------------------------------------------------
// Settings (prefilled from the command line, edited in the menu)
// ---------------------------------------------------------------------------

#[derive(Resource, Clone)]
struct Settings {
    players: usize,
    width: i32,
    height: i32,
    seed: u64,
}

fn now_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_secs())
}

fn parse_args() -> Settings {
    let mut cfg = Settings {
        players: 3,
        width: 16,
        height: 12,
        seed: now_seed() % 100_000,
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
            ("--help", _) => {
                println!("usage: beavy [--players N] [--width N] [--height N] [--seed N]");
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
    cfg
}

impl Settings {
    fn clamp(&mut self) {
        self.width = self.width.clamp(MIN_W, MAX_W);
        self.height = self.height.clamp(MIN_H, MAX_H);
        // Game::new needs 5 cells per player.
        let by_area = usize::try_from(self.width * self.height / 5).unwrap_or(0);
        self.players = self.players.clamp(MIN_PLAYERS, MAX_PLAYERS.min(by_area));
    }
}

// ---------------------------------------------------------------------------
// ECS state, resources and components
// ---------------------------------------------------------------------------

#[derive(States, Default, Debug, Clone, PartialEq, Eq, Hash)]
enum AppState {
    #[default]
    Menu,
    Playing,
    /// One-frame hop used to rebuild the board with the same settings.
    Restart,
}

#[derive(Resource)]
struct GameRes(Game);

#[derive(Resource, Default)]
struct Selected(Option<u32>);

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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    Dec(Field),
    Inc(Field),
    Start,
    Replay,
    Menu,
}

#[derive(Component)]
struct MenuButton(MenuAction);

#[derive(Component)]
struct ValueText(Field);

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
        .insert_resource(parse_args())
        .init_resource::<Selected>()
        .init_state::<AppState>()
        .add_systems(Startup, (spawn_camera, load_tank_assets))
        // Menu
        .add_systems(OnEnter(AppState::Menu), build_menu)
        .add_systems(
            Update,
            (menu_keys, sync_menu_values)
                .chain()
                .run_if(in_state(AppState::Menu)),
        )
        .add_systems(Update, handle_buttons)
        .add_systems(
            OnEnter(AppState::Restart),
            |mut next: ResMut<NextState<AppState>>| {
                next.set(AppState::Playing);
            },
        )
        .add_systems(OnExit(AppState::Menu), despawn_all::<MenuEntity>)
        // Match
        .add_systems(OnEnter(AppState::Playing), build_board)
        .add_systems(
            Update,
            (
                handle_input,
                sync_units,
                sync_hover,
                sync_hud,
                pulse_selected,
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

fn build_menu(
    mut commands: Commands,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut cameras: Query<&mut Transform, With<Camera2d>>,
) {
    if let Ok(mut w) = windows.single_mut() {
        w.resolution.set(MENU_W, MENU_H);
    }
    if let Ok(mut t) = cameras.single_mut() {
        t.translation = Vec3::ZERO;
    }

    let font = |size: f32| TextFont {
        font_size: size,
        ..default()
    };

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
            MenuEntity,
        ))
        .with_children(|root| {
            root.spawn((
                Text::new("BEAVY TANKS"),
                font(40.0),
                TextColor(Color::srgb(0.95, 0.85, 0.30)),
                Node {
                    margin: UiRect::bottom(Val::Px(10.0)),
                    ..default()
                },
            ));

            for (label, field) in [
                ("Players", Field::Players),
                ("Board width", Field::Width),
                ("Board height", Field::Height),
                ("Map seed", Field::Seed),
            ] {
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
                });
            }

            root.spawn(Node {
                margin: UiRect::top(Val::Px(16.0)),
                ..default()
            })
            .with_children(|row| {
                spawn_button(
                    row,
                    "START  (Enter)",
                    MenuAction::Start,
                    START,
                    240.0,
                    &font(26.0),
                );
            });

            root.spawn((
                Text::new("Keyboard: arrows change width/height, +/- players, S reroll seed"),
                font(14.0),
                TextColor(Color::srgb(0.55, 0.55, 0.6)),
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
    }
    settings.clamp();
}

/// Shared handler for every UI button (menu and end screen).
fn handle_buttons(
    mut settings: ResMut<Settings>,
    mut next: ResMut<NextState<AppState>>,
    mut q: Query<
        (&Interaction, &MenuButton, &mut BackgroundColor),
        (Changed<Interaction>, With<Button>),
    >,
) {
    for (interaction, button, mut bg) in &mut q {
        let is_start = matches!(button.0, MenuAction::Start | MenuAction::Replay);
        match *interaction {
            Interaction::Pressed => {
                *bg = BackgroundColor(BTN_PRESS);
                match button.0 {
                    MenuAction::Dec(f) => adjust(&mut settings, f, -1),
                    MenuAction::Inc(f) => adjust(&mut settings, f, 1),
                    MenuAction::Start => next.set(AppState::Playing),
                    MenuAction::Replay => next.set(AppState::Restart),
                    MenuAction::Menu => next.set(AppState::Menu),
                }
            }
            Interaction::Hovered => {
                *bg = BackgroundColor(if is_start { START_HOVER } else { BTN_HOVER });
            }
            Interaction::None => *bg = BackgroundColor(if is_start { START } else { BTN }),
        }
    }
}

fn menu_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut settings: ResMut<Settings>,
    mut next: ResMut<NextState<AppState>>,
) {
    if keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter]) {
        next.set(AppState::Playing);
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

fn sync_menu_values(settings: Res<Settings>, mut q: Query<(&ValueText, &mut Text)>) {
    if !settings.is_changed() {
        return;
    }
    for (field, mut text) in &mut q {
        text.0 = match field.0 {
            Field::Players => settings.players.to_string(),
            Field::Width => settings.width.to_string(),
            Field::Height => settings.height.to_string(),
            Field::Seed => settings.seed.to_string(),
        };
    }
}

// ---------------------------------------------------------------------------
// Board setup / teardown
// ---------------------------------------------------------------------------

fn build_board(
    mut commands: Commands,
    settings: Res<Settings>,
    mut selected: ResMut<Selected>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut cameras: Query<&mut Transform, With<Camera2d>>,
) {
    let game = Game::new(
        settings.width,
        settings.height,
        settings.players,
        settings.seed,
    );
    selected.0 = first_own_unit(&game);

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

    // Bases: coloured frame around the tile.
    for (i, player) in game.players.iter().enumerate() {
        commands.spawn((
            Sprite::from_color(player_color(i), Vec2::splat(CELL - 2.0)),
            Transform::from_translation(cell_center(player.base, 0.2)),
            BoardEntity,
        ));
        commands.spawn((
            Sprite::from_color(Color::srgb(0.22, 0.22, 0.22), Vec2::splat(CELL - 14.0)),
            Transform::from_translation(cell_center(player.base, 0.3)),
            BoardEntity,
        ));
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

/// First tank of the player to act, used as the default selection.
fn first_own_unit(g: &Game) -> Option<u32> {
    g.units.iter().find(|u| u.owner == g.current).map(|u| u.id)
}

/// Next own tank after `current` in unit order, wrapping around.
fn next_own_unit(g: &Game, current: Option<u32>) -> Option<u32> {
    let own: Vec<u32> = g
        .units
        .iter()
        .filter(|u| u.owner == g.current)
        .map(|u| u.id)
        .collect();
    let idx = current
        .and_then(|id| own.iter().position(|&x| x == id))
        .map_or(0, |i| (i + 1) % own.len().max(1));
    own.get(idx).copied()
}

fn handle_input(
    mut game: ResMut<GameRes>,
    mut selected: ResMut<Selected>,
    mut next: ResMut<NextState<AppState>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
) {
    if keys.just_pressed(KeyCode::KeyM) {
        next.set(AppState::Menu);
        return;
    }
    if game.0.result.is_some() {
        if keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter]) {
            next.set(AppState::Restart);
        }
        return;
    }

    // Mouse: select one of the current player's tanks, or deselect.
    if mouse.just_pressed(MouseButton::Left) {
        let g: &Game = &game.0;
        let picked = cursor_cell(&windows, &cameras)
            .filter(|&p| g.in_bounds(p))
            .and_then(|p| g.unit_at(p))
            .filter(|u| u.owner == g.current)
            .map(|u| u.id);
        if selected.0 != picked {
            selected.0 = picked;
        }
    }

    if keys.just_pressed(KeyCode::Escape) && selected.0.is_some() {
        selected.0 = None;
    }
    if keys.just_pressed(KeyCode::Tab) {
        selected.0 = next_own_unit(&game.0, selected.0);
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
        if let Some(unit) = selected.0 {
            Some(Action::Move { unit, dir })
        } else {
            game.0
                .log
                .push("Select one of your tanks first (click it).".into());
            None
        }
    } else if keys.just_pressed(KeyCode::KeyP) {
        Some(Action::Produce)
    } else if keys.just_pressed(KeyCode::KeyR) {
        if let Some(unit) = selected.0 {
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
    match game.0.apply(action) {
        Ok(_) => {
            // Turn passed to the next player; preselect one of their tanks.
            selected.0 = first_own_unit(&game.0);
        }
        Err(e) => game.0.log.push(format!("Not allowed: {e}.")),
    }
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
        spawn_tank(&mut commands, &assets, u);
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

/// Pulse the hull of the selected tank so the active player sees which tank
/// their keys will drive.
fn pulse_selected(
    selected: Res<Selected>,
    time: Res<Time>,
    mut hulls: Query<(&Hull, &mut Sprite)>,
) {
    let phase = (time.elapsed_secs() * 6.0).sin() * 0.5 + 0.5;
    for (hull, mut sprite) in &mut hulls {
        let base = player_color(hull.owner);
        sprite.color = if Some(hull.unit) == selected.0 {
            base.mix(&Color::WHITE, 0.7 * phase)
        } else {
            base
        };
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
    if selected.0.is_some() {
        selected.0 = None;
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
                spawn_button(
                    row,
                    "Play again (Enter)",
                    MenuAction::Replay,
                    START,
                    230.0,
                    &font(20.0),
                );
                spawn_button(row, "Menu (M)", MenuAction::Menu, BTN, 160.0, &font(20.0));
            });
        });
}

fn sync_hud(game: Res<GameRes>, selected: Res<Selected>, mut hud: Query<&mut Text, With<HudText>>) {
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
        let _ = writeln!(
            s,
            "Turn {}  |  {} ({}) to act",
            g.turn,
            cur.name,
            PLAYER_COLOR_NAMES[g.current % PLAYER_COLOR_NAMES.len()]
        );
    }

    let roster: Vec<String> = g
        .players
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if !p.alive {
                return format!("{}: out", p.name);
            }
            let (count, power) = g
                .units
                .iter()
                .filter(|u| u.owner == i)
                .fold((0, 0), |(c, p), u| (c + 1, p + u.strength));
            format!("{}: {}t/{}p", p.name, count, power)
        })
        .collect();
    s.push_str(&roster.join("  |  "));
    s.push('\n');

    if let Some(u) = selected.0.and_then(|id| g.unit(id)) {
        let _ = writeln!(
            s,
            "Selected: tank #{} at ({}, {}) strength {}",
            u.id, u.pos.x, u.pos.y, u.strength
        );
    } else if g.result.is_none() {
        s.push_str("Selected: none (click one of your tanks or press Tab)\n");
    }

    s.push_str("Blinking tank = selected (click or Tab to change) | Arrows move | P produce | R raise | Space pass | M menu\n");
    if let Some(last) = g.log.last() {
        s.push_str(last);
    }

    text.0 = s;
}
