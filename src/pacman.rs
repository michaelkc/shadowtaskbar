//! Classic Pac-Man chase, drawn with GDI primitives onto the shadow bar.
//!
//! The animation is a pure simulation: `tick` advances positions/eaten state,
//! `render` draws one frame into a cached back buffer. Both are driven from the
//! window proc, so there is no extra thread and no ownership to speak of.

use std::cell::UnsafeCell;
use std::f32::consts::PI;

use windows::Win32::Foundation::{COLORREF, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush, DeleteDC, DeleteObject,
    Ellipse, FillRect, GetStockObject, Polygon, SelectObject, HBITMAP, HDC, HGDIOBJ, NULL_PEN,
    SRCCOPY,
};
/// Timer period driving one animation frame. ~25fps, chunky like the arcade.
pub const TICK_MS: u32 = 40;
/// How often the whole chase repeats while the menu option is on.
pub const CYCLE_MS: u32 = 60_000;

// COLORREF is 0x00BBGGRR.
const COLOR_BLACK: COLORREF = COLORREF(0x0000_0000);
const COLOR_PACMAN: COLORREF = COLORREF(0x0000_FFFF); // #FFFF00
const COLOR_GHOST: COLORREF = COLORREF(0x00FF_2121); // #2121FF vulnerable blue
const COLOR_FLASH: COLORREF = COLORREF(0x00FF_FFFF); // white flash frames
const COLOR_EYE: COLORREF = COLORREF(0x0000_00FF); // #FF0000
const COLOR_DOT: COLORREF = COLORREF(0x00AE_B8FF); // #FFB8AE

const GHOST_SPEED: f32 = 84.0; // px/s, slow shuffle
const GHOST_PANIC: f32 = 104.0; // px/s, fleeing when pacman is on top of them
const PAC_SPEED: f32 = 92.0;
const PANIC_RANGE: f32 = 6.0; // in sprite widths
const SECS_PER_TICK: f32 = TICK_MS as f32 / 1000.0;

/// Frames one dot spends fading from black up to full brightness.
const DOT_FADE: f32 = 14.0;
/// Frames the fade start is staggered across, left to right, so the whole maze
/// lights up over roughly (DOT_STAGGER + DOT_FADE) * TICK_MS ~ 2.2 seconds.
const DOT_STAGGER: f32 = 42.0;

struct Dot {
    x: f32,
    y: f32,
    alive: bool,
    /// Frame this dot begins fading in on.
    appear: f32,
}

#[derive(PartialEq, Clone, Copy)]
enum Phase {
    /// Dots fading in left to right, everyone else still off-bar.
    Filling,
    /// Dots are lit; ghosts and pacman enter from the right.
    Chasing,
}

struct Anim {
    active: bool,
    phase: Phase,
    frame: f32,
    w: i32,
    h: i32,
    /// Sprite edge length in px; dots sit on a grid of this spacing.
    s: f32,
    dots: Vec<Dot>,
    ghosts: [f32; 3],
    pac: f32,
    /// Set for one frame when pacman eats a dot, to widen the mouth.
    chomp: bool,
}

/// The animation lives in a `static` so the window proc can reach it without
/// threading it through every call. Single-threaded by construction.
struct Shared(UnsafeCell<Anim>);

// SAFETY: only ever touched from the one thread running the message loop.
unsafe impl Sync for Shared {}

static PACMAN: Shared = Shared(UnsafeCell::new(Anim::new()));

/// Starts a chase on a bar of the given size. Returns false if one is already
/// running.
///
/// SAFETY: caller must be on the message-loop thread.
pub unsafe fn start(w: i32, h: i32) -> bool {
    let a = &mut *PACMAN.0.get();
    if a.active {
        return false;
    }
    a.begin(w, h);
    true
}

/// SAFETY: caller must be on the message-loop thread.
pub unsafe fn is_active() -> bool {
    (*PACMAN.0.get()).active
}

/// Advances one frame. Safe to call when idle (no-op).
///
/// SAFETY: caller must be on the message-loop thread.
pub unsafe fn tick(w: i32, h: i32) {
    let a = &mut *PACMAN.0.get();
    if !a.active {
        return;
    }
    if a.w != w || a.h != h {
        // Bar was resized (taskbar size change) - re-lay out the maze and keep going.
        a.w = w;
        a.h = h;
        a.layout();
    }
    a.step();
}

/// Repaints the bar into `dc`: solid black, plus the chase if one is running.
///
/// SAFETY: caller must be on the message-loop thread.
pub unsafe fn render(dc: HDC, w: i32, h: i32) {
    let a = &*PACMAN.0.get();

    let bb = &mut *BACKBUFFER.0.get();
    if !bb.ensure(dc, w, h) {
        return;
    }
    let mem = bb.dc;

    // Black backdrop every frame: the bar must read as pure black once the
    // chase is over, so nothing from the previous frame can linger.
    let black = CreateSolidBrush(COLOR_BLACK);
    FillRect(
        mem,
        &RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        },
        black,
    );
    let _ = DeleteObject(HGDIOBJ(black.0));

    if a.active {
        let lane = a.h as f32 / 2.0;
        a.draw_dots(mem);
        // The chase waits until the whole maze is lit - otherwise the ghosts
        // walk in over dots that are still fading in underneath them.
        if a.phase == Phase::Chasing {
            for g in a.ghosts {
                a.draw_ghost(mem, g, lane);
            }
            a.draw_pacman(mem, lane);
        }
    }

    let _ = BitBlt(dc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
}

/// Frees the cached GDI objects. Call from WM_DESTROY.
pub unsafe fn dispose() {
    (&mut *BACKBUFFER.0.get()).dispose();
}

struct Backbuffer {
    dc: HDC,
    bmp: HBITMAP,
    prev: HGDIOBJ,
    w: i32,
    h: i32,
}

struct BackbufferShared(UnsafeCell<Backbuffer>);

// SAFETY: only ever touched from the one thread running the message loop.
unsafe impl Sync for BackbufferShared {}

static BACKBUFFER: BackbufferShared = BackbufferShared(UnsafeCell::new(Backbuffer {
    dc: HDC(std::ptr::null_mut()),
    bmp: HBITMAP(std::ptr::null_mut()),
    prev: HGDIOBJ(std::ptr::null_mut()),
    w: 0,
    h: 0,
}));

impl Backbuffer {
    fn dispose(&mut self) {
        unsafe {
            if !self.prev.is_invalid() {
                let _ = SelectObject(self.dc, self.prev);
                let _ = DeleteObject(HGDIOBJ(self.bmp.0));
                let _ = DeleteDC(self.dc);
                self.prev = HGDIOBJ(std::ptr::null_mut());
                self.bmp = HBITMAP(std::ptr::null_mut());
                self.dc = HDC(std::ptr::null_mut());
                self.w = 0;
                self.h = 0;
            }
        }
    }

    unsafe fn ensure(&mut self, screen: HDC, w: i32, h: i32) -> bool {
        if w <= 0 || h <= 0 {
            return false;
        }
        if self.w == w && self.h == h && !self.dc.is_invalid() {
            return true;
        }
        self.dispose();
        self.dc = CreateCompatibleDC(screen);
        if self.dc.is_invalid() {
            return false;
        }
        self.bmp = CreateCompatibleBitmap(screen, w, h);
        if self.bmp.is_invalid() {
            let _ = DeleteDC(self.dc);
            self.dc = HDC(std::ptr::null_mut());
            return false;
        }
        self.prev = SelectObject(self.dc, HGDIOBJ(self.bmp.0));
        // Sprites are filled shapes with no outlines - the stock black pen would
        // otherwise ring every ghost and eye.
        let _ = SelectObject(self.dc, HGDIOBJ(GetStockObject(NULL_PEN).0));
        self.w = w;
        self.h = h;
        true
    }
}

/// Selects a solid brush, runs `f` with it selected, then restores and deletes.
/// The brush handle is passed in so callers can build it however they need.
unsafe fn with_brush<F: FnOnce()>(dc: HDC, color: COLORREF, f: F) {
    let brush = CreateSolidBrush(color);
    let old = SelectObject(dc, HGDIOBJ(brush.0));
    f();
    let _ = SelectObject(dc, old);
    let _ = DeleteObject(HGDIOBJ(brush.0));
}

impl Anim {
    const fn new() -> Self {
        Anim {
            active: false,
            phase: Phase::Filling,
            frame: 0.0,
            w: 0,
            h: 0,
            s: 16.0,
            dots: Vec::new(),
            ghosts: [0.0; 3],
            pac: 0.0,
            chomp: false,
        }
    }

    /// Kicks off a chase: dots laid out dark, ghosts queued just off the right
    /// edge with pacman trailing behind them. Nothing moves until the maze has
    /// finished fading in.
    fn begin(&mut self, w: i32, h: i32) {
        self.w = w;
        self.h = h;
        self.frame = 0.0;
        self.active = true;
        self.phase = Phase::Filling;
        self.chomp = false;
        self.layout();
        let s = self.s;
        for (i, g) in self.ghosts.iter_mut().enumerate() {
            // The first ghost leads (rightmost), the others queue up behind it
            // toward the left, all still off-bar.
            *g = -s * (1.0 + i as f32 * 1.2);
        }
        self.pac = -s * 5.5;
    }

    /// Rebuilds the dot grid for the current bar size.
    fn layout(&mut self) {
        let h = self.h as f32;
        // Sprites fill two thirds of the bar's height. The upper clamp has to
        // stay above that, or big bars get a stunted sprite instead.
        self.s = (h * 2.0 / 3.0).clamp(12.0, 256.0);
        let s = self.s;
        let cols = ((self.w as f32 / s).floor() as usize).max(1);
        let mut dots = Vec::with_capacity(cols);
        let y = h / 2.0;
        for c in 0..cols {
            dots.push(Dot {
                x: s * 0.5 + c as f32 * s,
                y,
                alive: true,
                // Left to right sweep across the bar.
                appear: c as f32 / cols as f32 * DOT_STAGGER,
            });
        }
        self.dots = dots;
    }

    fn step(&mut self) {
        self.frame += 1.0;

        if self.phase == Phase::Filling {
            if self.maze_lit() {
                self.phase = Phase::Chasing;
            } else {
                return;
            }
        }

        let s = self.s;
        let right_edge = self.w as f32 + s * 1.5;

        for i in 0..self.ghosts.len() {
            let near = (self.ghosts[i] - self.pac).abs() < s * PANIC_RANGE;
            let speed = if near { GHOST_PANIC } else { GHOST_SPEED };
            self.ghosts[i] += speed * SECS_PER_TICK;
            if self.ghosts[i] > right_edge {
                // Park off-bar; pacman follows them out, so the chase reads as
                // them escaping right rather than vanishing mid-bar.
                self.ghosts[i] = right_edge;
            }
        }

        self.pac += PAC_SPEED * SECS_PER_TICK;

        // Pacman is a 2s-wide circle with a mouth on his leading (right) side;
        // any dot he overlaps is one he just ate.
        let eat_x = s * 0.55;
        let mut chomp = false;
        for d in self.dots.iter_mut() {
            if d.alive && d.lit(self.frame) >= 1.0 && (d.x - self.pac).abs() < eat_x {
                d.alive = false;
                chomp = true;
            }
        }
        self.chomp = chomp;

        if self.pac > right_edge {
            self.active = false;
        }
    }

    unsafe fn draw_dots(&self, dc: HDC) {
        for d in &self.dots {
            if !d.alive {
                continue;
            }
            // Fade up out of black, left to right; the dot also grows slightly
            // as it lights so the sweep has some weight to it.
            let k = d.lit(self.frame);
            if k <= 0.0 {
                continue;
            }
            let r = self.s * (0.09 + 0.02 * k);
            with_brush(dc, dim(COLOR_DOT, k), || {
                let _ = Ellipse(
                    dc,
                    (d.x - r) as i32,
                    (d.y - r) as i32,
                    (d.x + r) as i32,
                    (d.y + r) as i32,
                );
            });
        }
    }

    /// Classic ghost: domed head, body, four feet on a two-frame skirt walk.
    /// Vulnerable ghosts flash between blue and white. The whole sprite is `s`
    /// tall and centered on the lane.
    unsafe fn draw_ghost(&self, dc: HDC, x: f32, lane: f32) {
        let s = self.s;
        let r = s / 2.0;
        let cx = x + r;
        let top = lane - r;
        let cy = top + r;
        // Hem sits high enough to leave a short skirt, so the dome reads as a head
        // rather than swallowing the whole sprite.
        let body_bottom = top + s * 0.74;

        let flashing = self.frame % 12.0 >= 6.0;
        let body = if flashing { COLOR_FLASH } else { COLOR_GHOST };
        let pupil = if flashing { COLOR_EYE } else { COLOR_GHOST };

        // Dome, left to right over the top, but only over the upper part of the sprite
        // so the ghost has a proper head/body split.
        let mut pts: Vec<POINT> = Vec::with_capacity(32);
        let dome_r = s * 0.5;
        let dome_cy = top + dome_r;
        for i in 0..=16 {
            let a = PI - i as f32 * PI / 16.0;
            pts.push(pt(cx + dome_r * a.cos(), dome_cy - dome_r * a.sin()));
        }
        pts.push(pt(cx + dome_r, body_bottom));

        // Four feet along the hem; `shift` flips the wave every four frames to give
        // the classic two-frame walk. Traced right to left so the outline
        // continues from the dome without crossing itself.
        let shift = if (self.frame / 4.0) % 2.0 < 1.0 { 0.0 } else { 0.5 };
        let w = r / 2.0;
        for i in 0..4 {
            let hi = cx + r - (i as f32 + shift) * w;
            pts.push(pt(hi, body_bottom));
            pts.push(pt(hi - w / 2.0, top + s));
            pts.push(pt(hi - w, body_bottom));
        }

        with_brush(dc, body, || {
            let _ = Polygon(dc, &pts);
        });
        // Eyes look down the direction of travel (left).
        with_brush(dc, COLOR_FLASH, || {
            for sign in [-1.0f32, 1.0] {
                let ex = cx + sign * r * 0.36;
                let ey = cy + r * 0.10;
                let _ = Ellipse(
                    dc,
                    (ex - r * 0.26) as i32,
                    (ey - r * 0.32) as i32,
                    (ex + r * 0.26) as i32,
                    (ey + r * 0.32) as i32,
                );
            }
        });
        with_brush(dc, pupil, || {
            for sign in [-1.0f32, 1.0] {
                // Pupils shifted right: the ghosts are running that way.
                let ex = cx + sign * r * 0.36 + r * 0.10;
                let ey = cy + r * 0.10;
                let _ = Ellipse(
                    dc,
                    (ex - r * 0.13) as i32,
                    (ey - r * 0.17) as i32,
                    (ex + r * 0.13) as i32,
                    (ey + r * 0.17) as i32,
                );
            }
        });
    }

    /// Pac-Man as a pie wedge: the gap between the sampled angles is his mouth,
    /// opening toward the right and cycling closed / half / wide.
    unsafe fn draw_pacman(&self, dc: HDC, lane: f32) {
        let s = self.s;
        let r = s / 2.0;
        let cx = self.pac + r;
        let cy = lane;
        // Three-frame chomp cycle: wide, half, closed. `mouth` is the half-angle of
        // the opening, in radians, always pointing along travel.
        let step: f32 = match (self.frame as u32) % 3 {
            0 => 0.62,
            1 => 0.28,
            _ => 0.04,
        };
        let mouth = if self.chomp { step.max(0.62) } else { step };

        // Angle 0 points right in screen space (y is negated below), so the wedge spans
        // everything except a gap centered on 0 for the mouth.
        let start = mouth;
        let end = 2.0 * PI - mouth;
        // The center point must be part of the path: Polygon auto-closes with a
        // straight line, so an arc-only point list would give a flat chord
        // across the mouth rather than a wedge cut into the circle.
        let mut pts = Vec::with_capacity(24);
        pts.push(pt(cx, cy));
        for i in 0..=20 {
            let a = start + (end - start) * i as f32 / 20.0;
            pts.push(pt(cx + r * a.cos(), cy - r * a.sin()));
        }

        with_brush(dc, COLOR_PACMAN, || {
            let _ = Polygon(dc, &pts);
        });
    }
}

impl Dot {
    /// How lit this dot is right now: 0 before its fade starts, 1 once done.
    fn lit(&self, frame: f32) -> f32 {
        ((frame - self.appear) / DOT_FADE).clamp(0.0, 1.0)
    }
}

impl Anim {
    /// True once every dot has finished fading in.
    fn maze_lit(&self) -> bool {
        self.dots
            .iter()
            .all(|d| d.lit(self.frame) >= 1.0)
    }
}

fn pt(x: f32, y: f32) -> POINT {
    POINT {
        x: x as i32,
        y: y as i32,
    }
}

/// Scales a color toward black by `k`, for the dot fade-in. COLORREF packs
/// 8 bits each of blue, green, red, so scaling the whole word is safe.
fn dim(c: COLORREF, k: f32) -> COLORREF {
    let scale = |v: u32| ((v as f32 * k) as u32).min(255);
    COLORREF(scale(c.0 & 0xFF) | (scale((c.0 >> 8) & 0xFF) << 8) | (scale(c.0 >> 16) << 16))
}