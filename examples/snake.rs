//! Snake, drawn with one subsurface per tile.
//!
//! Only the tiles that exist get a surface: the snake's body and the food.
//! Surfaces are reused rather than recreated — a step moves the tile that was
//! the tail to the front of the snake, so the number of surfaces tracks the
//! snake's length for the whole game.
//!
//! The board is a torus. The snake wraps off one edge and arrives on the other,
//! so `Pos::step` is a `rem_euclid` rather than a bounds check, and the edges
//! are not a way to lose. Running into itself is the only thing that is, and it
//! ends the game: the board is held still while the title flashes, then the
//! title asks for another round and nothing at all happens until a key says yes.
//! Then every tile surface is removed and a new game is laid out in the same
//! window.
//!
//! The game runs on a timer. That is the whole reason this example is async:
//! `Display::dispatch` sleeps inside the compositor's socket, so a blocking
//! loop has no way to wake up on its own and nothing can advance unless the user
//! presses a key. The library has no timer and no runtime in it — it hands over
//! the socket and the pieces of the read protocol, and the event loop here is
//! ordinary `tokio::select!` over that socket and a `tokio::time` interval.

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
use wayland_client::protocol::wl_buffer::WlBuffer;

/// Size of the playing field, in cells.
const GRID: i32 = 20;

/// Cell size used for the initial window, replaced by the compositor's choice.
const CELL: i32 = 24;

/// How long the snake waits between steps.
const STEP: Duration = Duration::from_millis(180);

/// One half of a title flash: the title alternates every `FLASH`.
const FLASH: Duration = Duration::from_millis(450);

/// How long the title flashes after a crash before it starts asking. A whole
/// number of halves, so the flash ends on a blank rather than mid-blink.
const FLASHES: Duration = Duration::from_millis(2_700);

/// The window title while the game is being played, with the score appended.
const TITLE: &str = "snake";

/// The title while it is flashing after a crash.
const OVER: &str = "GAME OVER!";

/// The title once the flash is over, which asks for the next game.
const RETRY: &str = "GAME OVER! — press any key to retry";

/// What the game is doing between steps.
#[derive(Clone, Copy)]
enum Phase {
    /// Moving.
    Playing,
    /// Crashed at this instant. The board is held still and the title flashes
    /// for [`FLASHES`].
    Flashing(Instant),
    /// The flash is over. Nothing happens until a key asks for another game.
    Waiting,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Pos(i32, i32);

impl Pos {
    /// One cell in `dir`, wrapping around the edges.
    ///
    /// The board is a torus: leaving one side arrives on the other, so `rem_euclid`
    /// rather than a bounds check, and the snake can run off the top and come
    /// back in at the bottom. The only way to lose is to run into the snake.
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

/// Just enough randomness to scatter the food.
impl Game {
    fn cell(&mut self) -> Pos {
        Pos(
            self.rng.random_range(0..GRID),
            self.rng.random_range(0..GRID),
        )
    }
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
    /// The real cell size, decided when the compositor first configures us.
    cell: i32,
    body: Vec<Tile>,
    dir: Pos,
    wanted: Pos,
    food: Option<Tile>,
    score: usize,
    phase: Phase,
    /// The last blink half written to the title, so a tick landing in the same
    /// half does not repeat the request.
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

    /// Put down one tile and fill it.
    fn spawn(&mut self, display: &mut Display, pos: Pos, ink: Ink) -> SurfaceId {
        let id = display
            .add_surface(SurfaceInfo {
                width: self.cell,
                height: self.cell,
                role: SurfaceRole::Subsurface {
                    parent: self.window,
                    x: pos.0 * self.cell,
                    y: pos.1 * self.cell,
                    // Synchronized, so a tile's cached state is applied when the
                    // window is committed — see `Game::place`, which does that.
                    sync: true,
                },
            })
            .expect("surface id space exhausted");

        display.commit(id, self.ink(ink));
        display.commit(self.window, &self.bg);
        id
    }

    /// Any cell that is neither snake nor food. The board can fill up, in which
    /// case the food lands on the head and the game is effectively over anyway.
    fn free_cell(&mut self) -> Pos {
        for _ in 0..(GRID * GRID) * 2 {
            let pos = self.cell();
            let taken = self.body.iter().any(|t| t.pos == pos)
                || self.food.as_ref().is_some_and(|f| f.pos == pos);
            if !taken {
                return pos;
            }
        }
        self.body[0].pos
    }

    /// Put a tile where it belongs, with the right colour, and repaint.
    ///
    /// The window is committed as well, which is what applies the tile's cached
    /// position and damage while it is in synchronized mode.
    fn place(&self, display: &mut Display, tile: &Tile, ink: Ink) {
        let surface = display.surface(tile.id).unwrap();
        surface.set_position(tile.pos.0 * self.cell, tile.pos.1 * self.cell);
        surface.commit(self.ink(ink));
        display.commit(self.window, &self.bg);
    }

    /// Lay out the opening position of a game. Called once the real cell size is
    /// known, and again for every replay.
    fn start(&mut self, display: &mut Display) {
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

        let pos = self.free_cell();
        let id = self.spawn(display, pos, Ink::Food);
        self.food = Some(Tile { pos, id });

        self.set_title(display);
    }

    /// Put the score in the window title, so it is readable from the task bar
    /// without watching the board.
    fn set_title(&mut self, display: &mut Display) {
        display.set_title(self.window, &format!("{TITLE} — score {}", self.score));
    }

    /// Whether the flash is over and the game is waiting to be asked for
    /// another round.
    fn awaiting_retry(&self) -> bool {
        matches!(self.phase, Phase::Waiting)
    }

    /// Start another game, if one was being offered. A no-op at any other time,
    /// so the key handler can pass every keypress straight through.
    fn retry(&mut self, display: &mut Display) {
        if self.awaiting_retry() {
            self.restart(display);
        }
    }

    /// Clear the board and start again in the same window.
    ///
    /// Every tile surface is removed rather than reused. A game that ended at
    /// length 30 would otherwise leave 27 surfaces behind for the next game,
    /// which grows without bound over a long session; the tiles are cheap to
    /// make and the game is short. The window itself is kept, so this does not
    /// flicker the toplevel or re-run the configure handshake.
    fn restart(&mut self, display: &mut Display) {
        for tile in self.body.drain(..) {
            display.remove_surface(tile.id);
        }
        if let Some(food) = self.food.take() {
            display.remove_surface(food.id);
        }
        self.start(display);
    }

    fn step(&mut self, display: &mut Display) {
        // Nothing to move until the board is laid out.
        if self.body.is_empty() {
            return;
        }

        // Hold the final board still long enough to see what went wrong, then
        // play again. Restarting on the tick after the pause elapses, so the
        // hold lasts a whole number of steps and never a fraction of one.
        //
        // The title flashes for as long as the hold lasts, so the state is
        // obvious even if the board is not the thing being looked at. It blinks
        // against an empty title rather than against the score, which reads more
        // like a game over than a number does. Driven from elapsed time rather
        // than a toggle, so the rate does not drift with `STEP` and a late tick
        // still lands on the right half.
        // Between games nothing moves, so that the final board is still there to
        // be looked at while the title makes its point.
        match self.phase {
            // Hold the board still and flash the title, so the state is obvious
            // even if the board is not the thing being looked at. It blinks
            // against an empty title rather than against the score, which reads
            // more like a game over than a number does. Driven from elapsed time
            // rather than a toggle, so the rate does not drift with `STEP` and a
            // late tick still lands on the right half.
            Phase::Flashing(died) => {
                if died.elapsed() < FLASHES {
                    // Which half of the blink this is. A half lasts `FLASH` but a
                    // tick lands every `STEP`, so the same half is usually seen
                    // two or three times over; remembering the last one keeps the
                    // request from being repeated unchanged. Lit on the even
                    // halves, so the notice is up on the first tick after the
                    // crash rather than a beat after it.
                    let half = died.elapsed().as_millis() / FLASH.as_millis();
                    if self.flash != Some(half) {
                        self.flash = Some(half);
                        let on = half.is_multiple_of(2);
                        display.set_title(self.window, if on { OVER } else { "" });
                    }
                    return;
                }
                // The flash has said its piece, so stop shouting and ask. Written
                // on this one tick, where the phase changes, rather than on every
                // tick of the wait that follows.
                self.phase = Phase::Waiting;
                display.set_title(self.window, RETRY);
                return;
            }
            // Waiting to be asked. `Game::retry` is what moves us out, and it is
            // called from the key handler rather than from here.
            Phase::Waiting => return,
            Phase::Playing => {}
        }

        // Doubling back into your own neck is not a legal move.
        if self.wanted != self.dir.opposite() {
            self.dir = self.wanted;
        }

        // The board wraps, so the edges are not a way to lose. Running into
        // yourself is the only thing that is.
        let next = self.body[0].pos.step(self.dir);
        if self.body.iter().any(|t| t.pos == next) {
            self.phase = Phase::Flashing(Instant::now());
            self.flash = None;
            // Leave a mark where it happened, so the board shows the mistake
            // through the pause. It rides on `body` so that the restart clears
            // it along with everything else.
            let id = self.spawn(display, next, Ink::Food);
            self.body.push(Tile { pos: next, id });
            println!("into yourself — score {}", self.score);
            return;
        }

        // Whether this move lands on the food decides whether the tail survives,
        // so settle that before touching the body. Eating is what makes the
        // snake longer, and that works by *keeping* the tail this turn.
        let ate = self.food.as_ref().is_some_and(|f| f.pos == next);

        // The old head stays in the body and stays where it is — it is now a
        // middle segment. Only its colour changes, so its surface is reused.
        let head = self.body.first().expect("the snake always has a head");
        self.place(display, head, Ink::Body);

        if ate {
            // Growing, so the tail stays and a new tile is needed for the head.
            let id = self.spawn(display, next, Ink::Head);
            self.body.insert(0, Tile { pos: next, id });

            self.score += 1;
            self.set_title(display);
            println!("ate — score {}, length {}", self.score, self.body.len());

            // Pick the new cell before dropping the old food, so the old cell is
            // still excluded and the food cannot land where it already was.
            let pos = self.free_cell();
            let mut food = self.food.take().expect("checked just above");
            food.pos = pos;
            self.place(display, &food, Ink::Food);
            self.food = Some(food);
        } else {
            // The tail is not destroyed, it is simply moved to the front and
            // recoloured, so the number of surfaces stays at the snake's length
            // for the whole game. The head handled above stays in the body, so
            // one segment is taken off the end and one is put on — a wash.
            let mut tail = self.body.pop().expect("the snake always has a head");
            tail.pos = next;
            self.place(display, &tail, Ink::Head);
            self.body.insert(0, tail);
        }
    }

    fn turn(&mut self, dir: Pos) {
        self.wanted = dir;
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
                title: "snake".into(),
            },
        })
        .expect("surface id space exhausted");

    // A fixed-size window, because the board is laid out once and never again.
    // The tiles are a grid of cells, so a resize would have to rebuild the whole
    // snake to stay square, and the game is short enough not to be worth it.
    //
    // The limits are double-buffered, so they have to be set before the first
    // commit that should carry them. That is the first commit answering the
    // configure, which happens further down — so setting them here is early
    // enough, whereas setting them after a commit would leave the window
    // resizable until the next one.
    display.set_size_limits(window, Some((board, board)), Some((board, board)));

    let mut game = Game::new(window, bg, inks);

    let mut reactor = Reactor::new(&display)?;

    // The creation requests are still sitting in the write buffer. Push them out
    // before waiting on anything: the compositor cannot configure a surface it
    // has not been told about, so it would never answer, the socket would never
    // become readable, and the loop would hang on its very first await.
    display.flush()?;

    let mut stepper = tokio::time::interval(STEP);
    // A step that arrives late should not buy a burst of catch-up steps.
    stepper.set_missed_tick_behavior(MissedTickBehavior::Delay);

    println!(
        "w/a/s/d, h/j/k/l, or the arrow keys to steer — q to quit, one step every {}ms",
        STEP.as_millis()
    );

    loop {
        // `recv` borrows the display for as long as it is awaited, so no other
        // branch here may touch it: the branches only record which one woke, and
        // the work follows. Biased, so a keypress is always read before the step
        // it might have been meant to steer — without it tokio picks a ready
        // branch at random, and a turn could be applied after the step that
        // consumed the tick it was meant for.
        let from_compositor = tokio::select! {
            biased;

            // The compositor has something to say: read it, run the handlers,
            // and write out whatever they queued.
            read = reactor.recv(&mut display) => {
                read?;
                true
            }

            // ...or it is time for the snake to move.
            _ = stepper.tick() => false,
        };

        if !from_compositor {
            // The first interval tick lands immediately, long before the
            // compositor's configure arrives, and a commit before that configure
            // is a protocol error. Ask the library rather than inferring it from
            // a field of our own.
            if display.is_configured(window) {
                game.step(&mut display);
            }
        }

        for event in display.events() {
            match event {
                Event::SurfaceEvent {
                    id,
                    event: SurfaceEvent::Configure { width, height },
                } => {
                    // Lay out once, at the first configure. A later resize is
                    // not handled: the tiles keep the cell size decided here, so
                    // a smaller window would clip the board. The window asks not
                    // to be resized, but the compositor may ignore that, so this
                    // is a fallback rather than a guarantee.
                    if id == window && game.body.is_empty() {
                        game.cell = (width.min(height) / GRID).max(1);
                        // The window has now been configured, so this is the
                        // first commit that may legally carry a buffer.
                        display.commit(window, &game.bg);
                        game.start(&mut display);
                    }
                }
                Event::SeatEvent { id: seat, event } => match event {
                    SeatEvent::Key { pressed, key, .. } => {
                        if !pressed {
                            continue;
                        }
                        // The preferred keysym, resolved to a character. This also
                        // covers the cursor keys, which have no character of
                        // their own and so need the library's fallback table.
                        let Some(c) = display.translate_char(seat, key) else {
                            continue;
                        };

                        match c.to_ascii_lowercase() {
                            'q' => return Ok(()),
                            // Between games, any other key takes the offer. Ahead
                            // of the steering keys, since those would otherwise be
                            // swallowed as an attempt to play on.
                            _ if game.awaiting_retry() => game.retry(&mut display),
                            'w' | 'k' => game.turn(Pos(0, -1)),
                            's' | 'j' => game.turn(Pos(0, 1)),
                            'a' | 'h' => game.turn(Pos(-1, 0)),
                            'd' | 'l' => game.turn(Pos(1, 0)),
                            '\u{2190}' => game.turn(Pos(-1, 0)),
                            '\u{2191}' => game.turn(Pos(0, -1)),
                            '\u{2192}' => game.turn(Pos(1, 0)),
                            '\u{2193}' => game.turn(Pos(0, 1)),
                            _ => continue,
                        }
                    }
                    // Movement is on a timer now, so a repeat rate has nothing to
                    // do here.
                    SeatEvent::RepeatInfo { .. } => {}
                },
            }
        }

        // Whatever this pass queued — a step's commits, the configure's first
        // buffer, a new title — has to reach the compositor before the loop waits
        // on it again, or the window being waited for never appears. `recv`
        // flushes as well, but only on the branch where it ran, so this is where
        // the rest of the pass goes out. Usually a no-op.
        display.flush()?;

        if display.should_close(window) {
            break;
        }
    }

    println!("score {}", game.score);
    Ok(())
}
