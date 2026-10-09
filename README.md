# ShadowTaskbar

A tiny Win32 appbar that reserves a strip of screen space along the bottom
edge of your primary monitor and paints it solid black — a "shadow" taskbar.
Can only run a single instance, there is an exit menu on right click.

## Why

If you have an OLED screen, you probably know that static screen elements, 
like the Windows task bar, can cause burn-in if they stay on screen all the
time.
Windows' auto-hide taskbar solves this problem, by removing the task bar 
from view when you are not activating it (with the mouse or keyboard).
However, running with auto-hide  _also_ frees up screen real estate where 
the task bar used to be.
That means the lower part of open windows gets hidden by the task bar when 
it does show.
That annoyed me - and so ShadowTaskbar was born to get the OLED benefit, 
while making sure open windows do not move into the newly freed screen real
estate.

ShadowTaskbar solves the issue by registering itself as a real Win32 appbar
(`SHAppBarMessage`), so the shell permanently reserves that space the same
way it would for the taskbar — nothing resizes into it, auto-hide or not —
and it paints the strip black, so even maximized static content avoids
lighting up that row of OLED pixels. 
Actual full screen apps (e.g. RDP) of course ignores task bars and use the 
entire screen - as they should. If you RDP to another Windows machine, you 
might want to run ShadowTaskbar on both; I do.

## What it does

- Registers an appbar (`ABM_NEW`) docked to the bottom edge of the primary
  monitor, matching the real taskbar's own reserved thickness (queried via
  `ABM_GETTASKBARPOS`, so it adapts to display scaling and Windows' small/
  default/large taskbar size setting) — overridable via the first CLI arg,
  e.g. `shadowtaskbar.exe 40`.
- Paints the window solid black, single instance, hidden from taskbar/alt-tab.
- Right-click for a small context menu with an Exit option and a
  "Pac-Man (every 60s)" toggle.
- Responds to taskbar changes (`ABN_POSCHANGED`) and fullscreen apps
  (`ABN_FULLSCREENAPP`) the same way the real taskbar does.

See [HANDOFF.md](HANDOFF.md) for implementation history and build notes.

## Build

```
build.cmd
```

or, if you already have a working MSVC linker on PATH:

```
cargo build --release
```

## Pac-Man mode

Toggle "Pac-Man (every 60s)" in the right-click menu and every minute the bar
plays a short chase, drawn entirely with GDI primitives (no image assets):

1. A single row of dots fades up out of black, left to right, over ~2 seconds.
2. Three vulnerable ghosts (flashing blue/white) and Pac-Man walk on from the
   left, so everything travels left to right. Their classic animations are on:
   the ghosts' two-frame skirt walk and colour flash, Pac-Man's three-frame
   chomp.
3. Pac-Man eats the dots as he passes them and chases the ghosts off the right
   edge of the bar.
4. Everyone leaves, the dots are gone, and the bar goes black again until the
   next cycle.

Sprites are sized to two thirds of the bar's height, so they scale with the
taskbar thickness. Turning the option off stops the 60s cycle; a chase already
in flight finishes rather than cutting off mid-frame.

## Limitations

Primary monitor only (no multi-monitor support), bottom
edge only (not configurable), the Pac-Man toggle is not persisted across
restarts.
Caveat emptor: This was vibe coded, but I did look over the code and it 
does not look like it is doing anything malicious. And it works for me.
