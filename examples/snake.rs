//! Snake, drawn with one subsurface per tile.
//!
//! Only tiles that exist get surfaces: the snake and the food. During normal
//! play, surfaces are reused rather than recreated — a step moves the tail
//! surface to the new head position, so the number of surfaces stays equal to
//! the snake's length.
//!
//! The board is a torus. Leaving one edge enters through the opposite edge, so
//! `Pos::step` wraps with `rem_euclid`. Running into the snake is the only way
//! to lose.
//!
//! Movement is timer-driven, because waiting on the compositor socket alone
//! cannot advance the game while no input is arriving. The library provides
//! neither a timer nor a runtime, so the loop uses `tokio::select!` to wait for
//! either compositor events or the next movement tick.
//!
//! After a crash the board is held still while the title flashes, then waits for
//! a key before starting a new round. Pausing uses the same state-machine path:
//! the board stays still and the title blinks until the pause key again.

use std::time::{Duration, Instant};

use anyhow::Result;
use aufhebung::{
    color::Color,
    display::Display,
    state::{Event, SeatEvent, SurfaceEvent},
    surface::{SurfaceId, SurfaceInfo, SurfaceRole},
    tokio::Reactor,
};
use rand::Rng as _;
use tokio::time::MissedTickBehavior;

// Re-exported rather than depended on directly: declaring our own
// `wayland-client` would have to match this crate's git revision exactly, or
// the `WlBuffer` types would not unify.
use aufhebung::wayland_client::protocol::wl_buffer::WlBuffer;

const GRID: i32 = 20;
const CELL: i32 = 24;

const STEP: Duration = Duration::from_millis(180);
const FLASH: Duration = Duration::from_millis(450);
const FLASHES: Duration = Duration::from_millis(2_700);

const TITLE: &str = "snake";
const OVER: &str = "GAME OVER!";
const RETRY: &str = "GAME OVER! — press any key to retry";
const PAUSED: &str = "PAUSED";
const WON: &str = "YOU WIN! — press any key to retry";

#[derive(Clone, Copy)]
enum Phase {
    Playing,
    Flashing(Instant),
    Waiting,
    Paused(Instant),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Pos(i32, i32);

impl Pos {
    fn step(self, dir: Pos) -> Pos {
        Pos(
            (self.0 + dir.0).rem_euclid(GRID),
            (self.1 + dir.1).rem_euclid(GRID),
        )
    }

    fn opposite(self) -> Pos {
        Pos(-self.0, -self.1)
    }
}

#[derive(Clone, Copy)]
enum Ink {
    Body,
    Head,
    Food,
}

struct Tile {
    pos: Pos,
    id: SurfaceId,
}

struct Inks {
    body: WlBuffer,
    head: WlBuffer,
    food: WlBuffer,
}

struct Game {
    window: SurfaceId,
    bg: WlBuffer,
    inks: Inks,

    /// Set from the first configure event.
    cell: i32,

    body: Vec<Tile>,
    dir: Pos,
    wanted: Pos,
    food: Option<Tile>,

    score: usize,
    phase: Phase,

    /// Last blink half written to the title.
    flash: Option<u128>,

    rng: rand::rngs::ThreadRng,
}

impl Game {
    fn new(window: SurfaceId, bg: WlBuffer, inks: Inks) -> Self {
        Self {
            window,
            bg,
            inks,
            cell: 0,
            body: Vec::new(),
            dir: Pos(0, 1),
            wanted: Pos(0, 1),
            food: None,
            score: 0,
            phase: Phase::Playing,
            flash: None,
            rng: rand::rng(),
        }
    }

    fn ink(&self, ink: Ink) -> &WlBuffer {
        match ink {
            Ink::Body => &self.inks.body,
            Ink::Head => &self.inks.head,
            Ink::Food => &self.inks.food,
        }
    }

    fn random_cell(&mut self) -> Pos {
        Pos(
            self.rng.random_range(0..GRID),
            self.rng.random_range(0..GRID),
        )
    }

    /// Find a cell not occupied by the snake or food.
    fn free_cell(&mut self) -> Option<Pos> {
        for _ in 0..(GRID * GRID) * 2 {
            let pos = self.random_cell();

            let occupied = self.body.iter().any(|tile| tile.pos == pos)
                || self.food.as_ref().is_some_and(|food| food.pos == pos);

            if !occupied {
                return Some(pos);
            }
        }

        // The random search is deliberately bounded. Fall back to a complete
        // scan so a nearly-full board does not depend on luck.
        for y in 0..GRID {
            for x in 0..GRID {
                let pos = Pos(x, y);

                let occupied = self.body.iter().any(|tile| tile.pos == pos)
                    || self.food.as_ref().is_some_and(|food| food.pos == pos);

                if !occupied {
                    return Some(pos);
                }
            }
        }

        None
    }

    /// Create a synchronized tile surface.
    fn spawn(&mut self, display: &mut Display, pos: Pos, ink: Ink) -> SurfaceId {
        let id = display
            .add_surface(SurfaceInfo {
                width: self.cell,
                height: self.cell,
                role: SurfaceRole::Subsurface {
                    parent: self.window,
                    x: pos.0 * self.cell,
                    y: pos.1 * self.cell,
                    sync: true,
                },
            })
            .expect("surface id space exhausted");

        display.commit(id, self.ink(ink));
        display.commit(self.window, &self.bg);
        id
    }

    /// Move and repaint an existing tile.
    fn place(&self, display: &mut Display, tile: &Tile, ink: Ink) {
        let surface = display.surface(tile.id).unwrap();

        surface.set_position(tile.pos.0 * self.cell, tile.pos.1 * self.cell);
        surface.commit(self.ink(ink));
        display.commit(self.window, &self.bg);
    }

    fn set_title(&mut self, display: &mut Display) {
        display.set_title(self.window, &format!("{TITLE} — score {}", self.score));
    }

    fn awaiting_retry(&self) -> bool {
        matches!(self.phase, Phase::Waiting)
    }

    fn start(&mut self, display: &mut Display) {
        debug_assert!(self.body.is_empty());
        debug_assert!(self.food.is_none());

        self.score = 0;
        self.phase = Phase::Playing;
        self.flash = None;

        let head = Pos(GRID / 2, GRID / 2);
        self.dir = Pos(0, 1);
        self.wanted = self.dir;

        for i in 0..3 {
            let pos = head.step(Pos(0, -i));
            let ink = if i == 0 { Ink::Head } else { Ink::Body };
            let id = self.spawn(display, pos, ink);

            self.body.push(Tile { pos, id });
        }

        let pos = self
            .free_cell()
            .expect("initial board must have room for food");

        let id = self.spawn(display, pos, Ink::Food);
        self.food = Some(Tile { pos, id });

        self.set_title(display);
    }

    fn restart(&mut self, display: &mut Display) {
        for tile in self.body.drain(..) {
            display.remove_surface(tile.id);
        }

        if let Some(food) = self.food.take() {
            display.remove_surface(food.id);
        }

        self.start(display);
    }

    fn retry(&mut self, display: &mut Display) {
        if self.awaiting_retry() {
            self.restart(display);
        }
    }

    fn step(&mut self, display: &mut Display) {
        if self.body.is_empty() {
            return;
        }

        match self.phase {
            Phase::Flashing(since) => {
                if since.elapsed() < FLASHES {
                    self.blink(display, since, OVER);
                    return;
                }

                self.phase = Phase::Waiting;
                display.set_title(self.window, RETRY);
                return;
            }

            Phase::Waiting => return,

            Phase::Paused(since) => {
                self.blink(display, since, PAUSED);
                return;
            }

            Phase::Playing => {}
        }

        if self.wanted != self.dir.opposite() {
            self.dir = self.wanted;
        }

        let next = self.body[0].pos.step(self.dir);
        let ate = self.food.as_ref().is_some_and(|food| food.pos == next);

        // Moving into the current tail is legal when the tail will move away
        // on this step. It is not legal when growing, because the tail stays.
        let hits_body = self
            .body
            .iter()
            .enumerate()
            .any(|(i, tile)| tile.pos == next && (ate || i != self.body.len() - 1));

        if hits_body {
            self.crash(display, next);
            return;
        }

        let head = self.body.first().expect("snake always has a head");
        self.place(display, head, Ink::Body);

        if ate {
            self.grow(display, next);
        } else {
            self.move_tail(display, next);
        }
    }

    fn crash(&mut self, display: &mut Display, pos: Pos) {
        self.phase = Phase::Flashing(Instant::now());
        self.flash = None;

        // Leave the collision visible until the round is restarted.
        let id = self.spawn(display, pos, Ink::Food);
        self.body.push(Tile { pos, id });

        println!("into yourself — score {}", self.score);
    }

    fn grow(&mut self, display: &mut Display, pos: Pos) {
        let id = self.spawn(display, pos, Ink::Head);
        self.body.insert(0, Tile { pos, id });

        self.score += 1;
        self.set_title(display);

        println!("ate — score {}, length {}", self.score, self.body.len());

        // If the board is full, there is nowhere left to put food.
        let Some(food_pos) = self.free_cell() else {
            self.phase = Phase::Waiting;
            display.set_title(self.window, WON);
            return;
        };

        let mut food = self.food.take().expect("ate implies food exists");
        food.pos = food_pos;
        self.place(display, &food, Ink::Food);
        self.food = Some(food);
    }

    fn move_tail(&mut self, display: &mut Display, pos: Pos) {
        let mut tail = self.body.pop().expect("snake always has a tail");

        tail.pos = pos;
        self.place(display, &tail, Ink::Head);
        self.body.insert(0, tail);
    }

    fn blink(&mut self, display: &mut Display, since: Instant, text: &str) {
        let half = since.elapsed().as_millis() / FLASH.as_millis();

        if self.flash == Some(half) {
            return;
        }

        self.flash = Some(half);

        let title = if half.is_multiple_of(2) { text } else { "" };

        display.set_title(self.window, title);
    }

    fn toggle_pause(&mut self, display: &mut Display) {
        match self.phase {
            Phase::Playing => {
                self.phase = Phase::Paused(Instant::now());
                self.flash = None;
            }

            Phase::Paused(_) => {
                self.phase = Phase::Playing;
                self.flash = None;
                self.set_title(display);
            }

            Phase::Flashing(_) | Phase::Waiting => {}
        }
    }

    fn turn(&mut self, dir: Pos) {
        if matches!(self.phase, Phase::Playing) {
            self.wanted = dir;
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut display = Display::new()?;

    let bg = display.add_color(Color::hex(0x18_1a_22));

    let inks = Inks {
        body: display.add_color(Color::hex(0x3c_d0_78)),
        head: display.add_color(Color::hex(0x9e_f0_c4)),
        food: display.add_color(Color::hex(0xe0_50_50)),
    };

    let board = GRID * CELL;

    let window = display
        .add_surface(SurfaceInfo {
            width: board,
            height: board,
            role: SurfaceRole::Window {
                title: TITLE.into(),
            },
        })
        .expect("surface id space exhausted");

    // The board is fixed-size, so resizing would mean rebuilding the tile
    // layout. The compositor may ignore these limits, so configure handling
    // below still uses the actual configured size.
    display.set_size_limits(window, Some((board, board)), Some((board, board)));

    let mut game = Game::new(window, bg, inks);
    let mut reactor = Reactor::new(&display)?;

    // Push the creation requests before waiting; the compositor cannot send a
    // configure event for a surface it has not received yet.
    display.flush()?;

    let mut stepper = tokio::time::interval(STEP);
    stepper.set_missed_tick_behavior(MissedTickBehavior::Delay);

    println!(
        "w/a/s/d, h/j/k/l, or the arrow keys to steer — \
         p to pause, q to quit, one step every {}ms",
        STEP.as_millis()
    );

    loop {
        // `recv` borrows the display while it waits, so neither select branch
        // mutates it. Work happens after the branch completes.
        //
        // Input wins when both branches are ready. This means a direction key
        // arriving at the same instant as a movement tick affects that tick
        // rather than the following one.
        let from_compositor = tokio::select! {
            biased;

            read = reactor.recv(&mut display) => {
                read?;
                true
            }

            _ = stepper.tick() => false,
        };

        if !from_compositor && display.is_configured(window) {
            game.step(&mut display);
        }

        for event in display.events() {
            match event {
                Event::SurfaceEvent {
                    id,
                    event: SurfaceEvent::Configure { width, height },
                } => {
                    if id == window && game.body.is_empty() {
                        game.cell = (width.min(height) / GRID).max(1);

                        // The configure has arrived, so the first buffer commit
                        // is now legal.
                        display.commit(window, &game.bg);
                        game.start(&mut display);
                    }
                }

                Event::SeatEvent { id: seat, event } => match event {
                    SeatEvent::Key { pressed, key, .. } => {
                        if !pressed {
                            continue;
                        }

                        let Some(c) = display.translate_char(seat, key) else {
                            continue;
                        };

                        match c.to_ascii_lowercase() {
                            'q' => return Ok(()),

                            _ if game.awaiting_retry() => {
                                game.retry(&mut display);
                            }

                            'p' | ' ' => {
                                game.toggle_pause(&mut display);
                            }

                            'w' | 'k' => {
                                game.turn(Pos(0, -1));
                            }

                            's' | 'j' => {
                                game.turn(Pos(0, 1));
                            }

                            'a' | 'h' => {
                                game.turn(Pos(-1, 0));
                            }

                            'd' | 'l' => {
                                game.turn(Pos(1, 0));
                            }

                            '\u{2190}' => {
                                game.turn(Pos(-1, 0));
                            }

                            '\u{2191}' => {
                                game.turn(Pos(0, -1));
                            }

                            '\u{2192}' => {
                                game.turn(Pos(1, 0));
                            }

                            '\u{2193}' => {
                                game.turn(Pos(0, 1));
                            }

                            _ => {}
                        }
                    }

                    SeatEvent::RepeatInfo { .. } | SeatEvent::Pointer(_) => {}
                },
            }
        }

        // `recv` flushes when it runs, but timer-driven passes do not go through
        // it. Flush whatever this pass queued before waiting again.
        display.flush()?;

        if display.should_close(window) {
            break;
        }
    }

    println!("score {}", game.score);
    Ok(())
}
