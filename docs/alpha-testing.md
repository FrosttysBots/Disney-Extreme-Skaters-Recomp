# Skating alpha: how to test

The Map Viewer's **Skate** mode is an alpha of the game's skating: the
physics, tricks and scoring follow the game's own code and scripts (see
[physics-notes.md](physics-notes.md)), on every level and with every
character. This page is for anyone trying it out.

## Getting started

1. Open the Map Viewer with your disc image (see the README's quick start).
2. Pick a level (the Hub is a good start; not the Skate Shop, which is
   the game's menu room) and a character.
3. Press **Skate**. AutoKick is on: the skater pushes by itself (untick
   it to push with W yourself).

## Controls

| Keyboard | Gamepad | What it does |
|---|---|---|
| A / D | Stick or D-pad left / right | Steer; in the air, spin |
| W / S | Stick or D-pad up / down | Push / brake (and pick tricks' directions); brake 1.4 s to step off the board |
| Space (hold, let go) | A | Crouch, then ollie |
| Q (+ direction) | X | Flip trick in the air |
| F (+ direction) | B | Grab in the air (hold to keep grabbing) |
| E (+ direction) | Y | Grind: ollie at a rail, then press it |
| Direction then E on a rail or lip | Direction then Y | Switch to that direction's grind or lip trick (another trick in the combo) |
| W then S | Up then down | Manual (balance with W / S) |
| F + direction | B + direction | Manuals 2 to 5, on the ground or from a manual |
| E as you launch off a quarter pipe's lip | Y | Lip trick on the coping (balance with A / D) |
| R as you land from vert | Shoulder buttons | Revert (keeps the combo going into a manual) |
| R held going up a quarter pipe | Shoulder buttons held | Spine transfer over to the ramp behind |
| W held going up a quarter pipe | Stick up held | Launch out over the lip instead of straight up |
| R tapped on the ground | Shoulder button tapped | 180 slide (A or D for backside/frontside), riding on the other way round |
| Two directions, then Q (or E in a manual) | Two directions, then X (or Y) | Special trick, when the special meter is full |
| Tab | | Jump to the next spawn point |
| Esc | | Stop skating |

A gamepad rumbles where the game's does: ollies, landings, grinds (a
buzz while on the rail), reverts, flails and bails (the Rumble box turns
it off).

On screen: the score and the special meter (top right), the combo and the
balance meter (bottom), and in the panel a status line with where the
skater is and what it's doing.

## What to try

- Pushing, steering, ollies and flips/grabs around a level.
- Grinding rails (each direction is a different grind), and balancing;
  grinds throw sparks off the board (slides don't).
- Manuals and branching between them, chaining grind → manual → grind.
- Quarter pipes: vert air, lips on copings that have a rail, reverts,
  and spine transfers (hold R going up; Camp's half pipes have spines).
- Filling the special meter and doing your character's specials.
- Gaps: jumps, grinds and lines the levels name and score (shown in the
  combo), like the Hub's Chain Link Gap or Camp's Tent Gap.
- Teleporters and water: falling in the Hub's harbour puts you back on
  the dock, as in the game.
- Coming down a quarter pipe or landing a 180: you turn round and ride
  switch (the character mirrored, as the game shows it).
- Sound: the game's own effects for each surface (rolling, ollies,
  landings, grinds), bails, gaps and slides; the characters' own lines
  when they bail or start a trick; "Special Trick" and its sound for
  specials; the coping knock going into a lip; the level's ambience; and the
  soundtrack (the Music box in the panel).
- Every level, and a few characters (each has its own tricks and stats).

## Known differences from the game

- Not yet: wall rides (the game barely uses them), skitching, the
  Simplified controls, goals and gameplay modes, and the level's
  other triggers (breakables, goal pickups).
- Approximations of our own: the camera follows the game's camera
  update (a lagging focus, turning, the quick swing after vert, zooming
  for big air tricks, grinds and lips) but sees past walls its own way
  (rising over them, holding back from swinging into a ramp, pulling
  in), lips start at coping rails in vert air (the
  game checks four angles), a skater barely rolling into a wall just stops
  against it, ground is found across hairline cracks in the levels'
  collision, and the skater is put back at the nearest spawn when it
  falls out of a level (the game uses out-of-bounds triggers).
- Skating starts from the character's spot rather than the game's level
  start, and goals, pedestrians' reactions and moving objects don't
  interact with the skater.

## Reporting a problem

Say which level and character, what you did (the keys or buttons), and
copy the status line from the panel (where the skater was and what it was
doing). Falling through the map, getting stuck, or the camera ending up
somewhere odd are the most useful things to report.

Developers can reproduce reports with
`cargo run --release -p desa_viewer --example skate_check -- extracted <level>`
(random skating with checks for falling through the ground, getting stuck
the camera behind walls or shaking; `PROBE=x,y,z` lists the collision
around a spot), and screenshot a scripted run with `--skate` and `--skate-keys`.
