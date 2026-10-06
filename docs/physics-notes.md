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
- **Ground following** (0x800F52AC, 549 instructions): casts from
  `Physics_Ground_Snap_Up` above (the `_SKITCHING` values while
  skitching), compares the new ground normal with the current one, and
  when moving onto ground that falls away by more than
  `Ground_stick_angle` (30, in degrees; `Ground_stick_angle_forward`, 60,
  while skater `+0x804` is set) it doesn't stick: the `GroundGone`
  exception fires and the skater is in the air. Otherwise it allows a
  snap down of `Physics_Ground_Snap_Down` plus the distance travelled
  times the tangent of the angle between the normals. Ported: the stick
  angle. Not yet: the extra snap distance, the forward angle, and what
  `+0x804` is. It also leads to `Normal_Lerp_Speed` (tilting). The game also
  has a `moon_gravity` cheat (it multiplies air gravity).

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

## Next

Read the rest of the on-ground update and its helpers, then the air
update, and check the port frame by frame against Dolphin (its debugger
can break on these addresses and show the skater's velocity at
`[[skater]+0x698]+0x34`).
