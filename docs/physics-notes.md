# Skater physics in `main.dol`: notes

What has been read from the GameCube executable (US, `GEXE52`) about the
skater's physics, for porting it exactly. Addresses are in memory
(`.text1` is at file offset `0x2E0`, loaded at `0x80003AA0`).

## Finding the code

The engine reads every physics constant by the checksum of its name
(`PHYSICS.q`), so the checksums appear as `lis`/`ori` constants next to
the code that uses them. Searching the executable for those constants
finds the code; useful hits:

| Constant | Used at |
|---|---|
| `Physics_Air_Gravity` | 0x800FC664, 0x800FC688 (air gravity) |
| `Physics_Ground_Gravity` | 0x800FB4C8 (on-ground update), 0x80118C90 |
| `Skater_Max_Standing_Kick_Speed_Stat` | 0x800F4464 (can push) |
| `Physics_Standing_Acceleration_Stat` | 0x800F45F0 (push) |
| `Skater_Max_Speed_Stat` | 0x800F48A8 |
| `Physics_Ground_Rotation` | 0x800ED8AC, 0x800ED8F8 (steering) |
| `Physics_Air_Rotation_stat` | 0x800EE654 and three more (air spins) |

## Helpers

| Address | What it does |
|---|---|
| 0x80030E2C | Reads a global float by checksum (`r3`) |
| 0x800F6020 | Reads a stat-scaled constant (`r4`) for the skater (`r3`) |
| 0x80163710 | Square root (used for vector lengths) |
| 0x800F44B0 | Is the skater crouched? |
| 0x80030EA4 | Reads a global float by name (a string, checksummed first) |
| 0x800F46C4 | Quadratic drag: `v -= v * |v| * k * 60 * dt` |
| 0x800F4AC0 | Linear friction: slows `v` by `k * dt` along itself, never reversing it |

## The skater object

`r3` in these functions is the skater. Its physics component is the
pointer at `+0x698`; in that component, `+0x34` is the velocity (x, y, z,
w), `+0x64` the facing vectors, and `+0xE4` the frame time. The ground
normal is at skater `+0x3A10`; `+0x28CC` holds a state checksum the
on-ground update switches on.

## Confirmed behavior

- **Air gravity** (0x800FC634): `Physics_Air_Gravity /
  Physics_Air_hang_Stat`, or `/ Physics_Vert_hang_Stat` while in vert air
  (skater `+0x3638` or `+0x36EC` set); multiplied by another constant when
  a flag is set (a cheat, by the look of it). Ported.
- **Can push** (0x800F43F0): only while the speed (length of the velocity)
  is at most the standing or crouched kick-speed stat. Ported.
- **Push** (0x800F44CC): velocity += direction * acceleration * frame
  time, where the direction is the current velocity normalized (or the
  facing when stopped) and the acceleration is the standing or crouched
  acceleration stat. Ported.
- **On-ground update** (0x800FB3E4, 1,172 instructions): builds gravity
  (0, `Physics_Ground_Gravity`, 0), removes its component into the ground
  (0x80009AC0 with the ground normal), adds the rest times the frame time
  to the velocity, then by state brakes (0x800F418C) or pushes. So slope
  gravity acts along the whole ground plane, sideways too; the port only
  applies it along the facing so far. It calls about 35 helpers still to
  be read. Next in it, behind a debug flag (`WalkTest`, `skitch_speed_match`
  are other such names), come blocks that halve the speed or add 200 in
  a direction; then it works out the proposed position (position +
  velocity * dt, at skater `+0x39A8`) for the collision checks.
- **Drag and friction** (0x800F4CF0): while the push button is held (skater
  `+0x3A38`), or another flag is set, or a friction value at `+0x3A58`
  differs from `Physics_Rolling_Friction`: quadratic drag with
  `Physics_Crouched_Air_Friction` or `Physics_Standing_Air_Friction`
  (0x800F49B8), then linear friction with the `+0x3A58` value times 60
  (0x800F4C9C, unless 0x800F3564 says otherwise). Ported: the drag while
  pushing. Not yet: when `+0x3A58` changes (terrain, perhaps).
- **Along the board** (0x800F4D5C): the velocity is turned to the facing,
  keeping its length and its sign, so the skater can roll backwards.
  Ported.
- **Speed limits** (0x800F4834), on horizontal speed only (vertical speed
  is set aside and put back): above `Skater_Max_Max_Speed_Stat` it's
  clamped, above `Skater_Max_Speed_Stat` it gets quadratic drag with
  `Physics_Heavy_Air_Friction`. A timer at skater `+0x3594` (a speed
  boost) swaps in other limits while it runs. Ported, without the boost.
- **Steering** (0x800ED848): left turns at `+Physics_Ground_Rotation`,
  right at minus it (radians a second, times the frame time), or
  `Physics_Ground_Sharp_Rotation` while the button that also stops pushes
  (skater `+0x82C`, probably brake) is held. Below a speed of 10 the rate
  is scaled by how long the turn has been held, up to 600 ms
  (0x800EBFF0 gives a button's held time). The velocity turns with the
  board. It also reads `cess_turn_min_speed` (cess slides). Ported.
- **Air spins** (0x800EE4CC): left or right spins at
  `Physics_Air_Rotation_stat`; in a tap-turn mode (skater `+0x3A40`) each
  tap queues half a turn (pi, at `+0x3A48`) taken at
  `Physics_air_tap_turn_speed_stat`. Ported: the held spin.
- **Braking** (0x800F418C, `Physics_Brake_Acceleration` by name): unless
  0x800F34D0 says it can't, the velocity is slowed by the brake rate times
  the frame time along itself; below twice that, or if it would reverse,
  it stops. Ported.
- **Ground following** (0x800F52AC, 549 instructions): a line down the
  skater's up axis (matrix row `+0x54`) from `Physics_Ground_Snap_Up`
  above the new position (the `_SKITCHING` value while skitching) to 200
  below. No hit, or a wall (by 0x800F66F0; a wall at or above the feet
  pushes the skater to the hit plus its normal first), and the ground is
  gone. Otherwise, with the facing (`+0x64`, not the velocity) flattened
  onto the new face and onto the current ground (`+0x3A10`), keeping
  length (0x80009B3C), and `cos` their dot product:
  - if the facing points along the new normal (the ground falls away
    ahead) and `0 < cos < cos(Ground_stick_angle)` (30 degrees;
    `Ground_stick_angle_forward`, 60, while `+0x804` is set), it doesn't
    stick;
  - if the skater is above the face, it only snaps down as far as the
    distance moved this frame times `tan(acos(cos))`, or
    `Physics_Ground_Snap_Down` (8.2) if that's more;
  - sticking, 0x800F4ED0 makes the face's normal the ground normal and
    the matrix's up at once (the drawn normal at `+0x3A00` eases over,
    `+0x3A34` reset to 1, `Normal_Lerp_Speed`), and the skater goes to the
    hit point. Then trigger events (types 4 and 5) for leaving and
    entering trigger faces, and the score.
  - not sticking: 0x800F612C(skater, 1) puts it in the air and the
    `GroundGone` exception fires.
  Ported: all but skitching, the forward angle, the events and how the
  drawn tilt eases. The game also has a `moon_gravity` cheat (it
  multiplies air gravity).
- **The ollie** (0x800F62C4, the scripts' `Jump` command): the jump speed
  is `min + (max - min) * tense / skater_max_tense_time`, where `tense`
  is the crouch time (skater `+0x3478`, capped at the max, 200 ms) and
  min and max are `Physics_Jump_Speed_min_stat` and
  `Physics_Jump_Speed_stat` (or the `Boneless` ones with a
  `BonelessHeight` parameter, or the `air` ones for jumps in the air).
  These are read by name, spelled `..._stat`, which is why searching for
  the `..._Stat` checksums found nothing. On the ground, moving down, it's
  added along the ground's normal; otherwise downward speed is dropped
  and it's added straight up. Upside down (vert) there's more. Ported:
  the ground ollie.

- **Walls on the ground** (0x800F7D38, from the ground update): a line
  `Skater_First_Forward_Collision_Height` (8.1) up the skater's up axis,
  from last frame's position to this frame's and
  `Skater_First_Forward_Collision_Length` (10) on, ignoring
  non-collidable faces. 0x800F66F0 sorts what it hits by the face's
  flags: skatable or vert is ground, not-skatable or wall-ridable is a
  wall, and otherwise it's a wall if its normal's Y is under
  `sin(Wall_Non_Skatable_Angle)` (25 degrees). Ground ahead (not
  perpendicular to the ground underfoot) moves the skater to the hit
  plus 0.1 along its normal, which is how it gets onto a ramp's curve.
  A wall goes to 0x800F6F98 (after a script event, type 6):
  - 0x800F6860 takes the angle between the skater's side axis (matrix
    row at `+0x44`) and the wall's normal, folded to within 90 degrees,
    and turns the velocity and the skater's matrix about Y by it times
    `Wall_Bounce_Angle_Multiplier` (1.1).
  - Hit more head-on than `Wall_Bounce_Dont_Slow_Angle` (30 degrees),
    the speed is scaled by `1 - (angle - 30) / (90 - 30)`: head-on stops.
  - On the ground, faster than `Wall_Bounce_Dont_Flail_Speed` (100), the
    skater plays `FlailLeft` or `FlailRight` (by the angle's sign and
    `+0x362C`, which may be the stance) and 0x8010E340 is called with the
    speed (a sound, perhaps).
  - The skater is put at the hit point, back down to its feet, plus 6
    along the wall's normal. Then a second line along the new direction:
    if that hits a wall too and `+0x37BC` is set (a corner?), it turns by
    180 degrees less the bounce.
  Ported: all but the corner case, the script event and the sound; the
  bounce's sign is chosen to turn away from the wall rather than read
  from the matrix. The air version is 0x800F847C (it reads
  `Skater_Min_Distance_To_Wall`), called from what looks like the air
  update, 0x800FC7F8.

- **The air update** (0x800FC7F8, about 1,250 instructions, much of it
  for vert air): gravity is a vector at skater `+0x5A8`, `(0, g, 0)` from
  0x800FC634; spinning is 0x800EE4CC. The step is exact for a thrown
  body: `position += v * dt + g * dt * dt / 2`, then `v += g * dt` (plus
  `+0xC4`, which looks like the motion of whatever it took off from).
  Ported.
- **Walls in the air** (0x800F847C): the same knee-height line as on the
  ground. A face that isn't a wall and is close to the last ground's
  normal (dot at least 0.8) or faces up (Y at least 0.5) is left for
  landing. A skatable face otherwise is tried as a ledge: up by
  `Physics_Air_Snap_Up`, then back down 2 at a time (not ported). With
  `+0x3638` set (vert air, perhaps) the skater is put
  `Skater_Min_Distance_To_Wall` out from the wall and its velocity
  flattened against it. Otherwise, after 0x800FDB6C (which may start a
  wall ride) and the script event:
  - a near-vertical wall (normal Y under 0.05) calls 0x8010E340 with the
    speed, like the ground's bonk;
  - unless the wall faces down (normal Y at most -0.1) the vertical
    speed is set aside; the velocity is flattened against the wall, gets
    a tenth of its length along the normal, and the vertical speed comes
    back;
  - the facing is flattened against the wall too, nudged 0.05 out and
    the matrix rebuilt;
  - the skater is put at the hit, back down to its feet, plus
    `Skater_Min_Distance_To_Wall` (8) along the normal; a moving
    object's motion is added.
  Ported: the last case, without the event, sound or moving objects.

- **Landing** (the end of 0x800FC7F8): a line from last frame's position
  to this one's (plus a moving object's motion). If it hits and the
  skater is rising faster than 10 or the face is nearly vertical (normal
  Y under 0.1), it first tries 0x800F81A8, the ledge pop: a line straight
  down from `Physics_Air_Snap_Up` (15) above the higher of the two
  positions to the lower; ground facing up there (normal Y over 0.5),
  reachable by a clear line from 15 above the old position, takes the
  skater, at the hit plus the normal and 0.1 higher. Otherwise the
  skater goes to the hit plus the normal, and:
  - not a wall (by 0x800F66F0, so vert faces count): it lands. The
    velocity loses its part along the normal (0x80009AC0) and stops
    below 10; the ground normal (`+0x3A00`, `+0x3A10`, `+0x3A20`) and
    the matrix's up become the face's normal and the matrix is
    re-orthonormalized around it, keeping the facing. Then the `Landed`
    event, sounds, the score (0x8011DE34) and a script event, type 3.
  - a wall: the velocity and the facing are turned along it keeping
    their length (0x80009B3C), and the skater moves out a further
    `Skater_Min_Distance_To_Wall`.
  Ported, without the events, sounds and moving objects.

## Next

Read the rest of the on-ground update and its helpers, then the air
update, and check the port frame by frame against Dolphin (its debugger
can break on these addresses and show the skater's velocity at
`[[skater]+0x698]+0x34`).
