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

- **Finding a rail** (0x800E52C4, called from 0x801078A8 with the move
  from last frame's position to this one's): every rail node with a next
  node whose box is within `Rail_Max_Snap` (40) of the move's; the
  closest points between the move and the segment (0x80003CE0); with
  `cos` the absolute dot product of their directions, the score is
  `distance * (1.122 - cos)` (doubled when 0x800E4E48, perhaps which side
  of the rail, disagrees with what's asked) and the lowest wins, if
  `distance * (2 - cos)` is within `Rail_Max_Snap`; if the best is out of
  reach, nothing is found unless a later rail scores better. The caller
  passes a tolerance of 1 (any angle) and first checks three timers
  against `Skater_regrind_time` (500 ms). When no level rail is found it
  tries the rails of moving objects.
- **Getting on** (0x80108470; it also starts lip tricks on vert): a line
  from the skater to the rail point must not hit anything more than 6
  short of it. A standing skater takes its facing as velocity; faster
  than 10, the velocity is made horizontal keeping its length
  (0x80009B3C), then projected on the rail (0x80009D00) keeping the
  vertical speed aside, and `Rail_Speed_Boost` (150) is added along the
  way it's going. The skater moves to the rail point, state 4.
- **Grinding** (0x80100A70, 2,600 instructions with balance): gravity
  `(0, Physics_Rail_Gravity, 0)` (-2000) projected on the rail is added
  each frame; turning round flips the fakie flag (`+0x3588`). Past a
  node it carries on to the next one (`+0x34`, or `+0x38` going
  backwards) unless the corner is sharper than `Rail_Corner_Leave_Angle`
  (50 degrees); the end of the rail drops it off (`OffRail`). Jumping
  (0x800F5B40) turns the velocity by `Rail_Jump_Angle` (15 degrees)
  towards the held direction and blocks the rail for
  `Rail_minimum_rerail_time` (500 ms).
  Each frame ends with a line from 1 above the old position to 1 above
  the new one and 6 on (0x801032E4): hitting anything (the ground where
  a rail dips into it, a wall) puts the skater back at the old position,
  1 higher, with its velocity flattened against what it hit, in the air
  (`OffRail`).
  Ported: all of the above for level rails, without balance, bails, the
  side check, moving objects' rails, lip tricks and the events. The
  grind button is the viewer's guess (the game's call on the ground
  depends on an argument not yet traced).

- **Vert air.** State 2 (0x800FE664) is wall riding (`Wall_Ride_*`),
  not vert: vert air is the air state with a flag at `+0x36E8`. It's set
  by 0x800F140C (about 1,800 instructions), called from the ground
  following when the ground is gone off a vert face (`+0x3638`) and
  from the air update. That function looks for where the skater will
  come down (columns of lines, `SkaterAwardTransfer` when it's another
  ramp: it then picks the horizontal speed that lands it there, if it's
  no faster than it's going) and otherwise turns the velocity straight
  up keeping its length (0x80009C70) when its direction and the ramp's
  disagree, and sets the spin axis (`+0xD4`). The air update then keeps
  the skater in line with the take-off (`+0x3420` to `+0x344C`), and
  pushes it `Physics_Vert_Push_Out` (3) out of faces it meets.
  Ported, simply: off a vert face going up, the speed away from the
  ramp is turned up and along it keeping its length, the skater starts
  3 out from the lip and stays in the ramp's vertical plane, and the
  ramp is left to landing; it comes back down fakie, as without a spin.
  Not ported: transfers, the spin axis, `skater_autoturn_vert_angle`,
  `Physics_Vert_hang_Stat`. On the Hub's quarter pipes
  (`cargo run --release -p desa_viewer --example vert_ramps`), 24 of the
  25 runs that leave the lip come back down onto the ramp (4 before).
- **Wall rides** (state 2, 0x800FE664; started by 0x800FDB6C from the
  air wall code) need the face flagged wall-ridable, the grind button
  held or pressed within `Wall_Ride_Triangle_Window`, `Wall_Ride_Delay`
  since the last, `Wall_Ride_Min_Speed` along the wall, a wall no more
  overhanging than `Wall_Ride_Upside_Down_Angle`, a glancing hit (under
  `Wall_Ride_Max_Incident_Angle`) and an upright skater
  (`Wall_Ride_Max_Tilt`); the ride uses `Wall_Ride_Gravity` and jumps off
  with `Wall_Ride_Jump_Out_Speed` and `Wall_Ride_Jump_Up_Speed`. But only
  11 faces in the whole game are wall-ridable (all in the canyon:
  `cargo run --release -p desa_viewer --example face_flags`), and no
  character has the `WallRide*` animations the `WallRide` script plays:
  it's left over from the engine. Not ported.
- **Balance** (manuals, grinds, lips): 0x800CAA50 starts a balance trick,
  0x800CAD30 updates it, called from the ground, rail and lip updates.
  Each change is scaled by the frame's length in 60ths of a second
  (0x800E8368). Per frame: the cheese drains by `Cheese / CheeseFrames`;
  with `instability = Instable_Base + time * Instable_Rate`, the angle
  grows by `angle * Lean_Gravity_Stat * instability` and moves by
  `speed * instability`. Button A (up in a manual, right on a rail)
  takes `Lean_Acc` off the speed, B adds it, once both have been let go
  since the start; for `BalanceSafeButtonPeriod` (1 s) a button that
  would push the lean further over does nothing. With neither held, a
  speed under `Lean_Min_Speed` becomes a random amount up to
  `Lean_Rnd_Speed` the same way, and otherwise gains up to 0.5 more.
  Past `Lean_Bail_Angle` (4000, on a meter of 4096) it fires
  `OffMeterTop` or `OffMeterBottom` and resets. Starting: a lean speed of
  `Repeat_Min` either way at random, or the last trick's times
  `Repeat_Multiplier` in a combo; the angle keeps
  `Lean_Repeat_Multiplier` of the last plus the cheese left over, and the
  cheese becomes `Cheese` (500 on rails, 0 in manuals). The scripts give
  the buttons (`DoBalanceTrick ButtonA = Up ButtonB = Down` for manuals,
  Right and Left for grinds and lips) and what falling off does: a
  manual bails off the top (`BailManual`) and sets down off the bottom
  (`ManualLand`); a grind falls to that side (`SkateInOrBail`), skating
  in onto ground beside the rail if there is any (`SkateInAble`, turning
  30 degrees) and bailing otherwise (`FiftyFiftyFall`). Manuals start
  with up then down (or down then up) within 400 ms (`ManualTricks`).
  Ported: all of that, the combo carrying the lean over until a landing
  without a manual; `SkateInAble` approximated, and bails lasting their
  fall and get-up animations. Not ported: lips, the other control scheme, special manuals.
- **Tricks** are scripts. `AirTricks` (`airtricks.q`) maps a button and a
  direction (`AirTrickLogic`, within 400 ms) to a slot (`Air_SquareD`);
  the profile's `default_trick_mapping` (`JessieTricks`) puts a trick in
  each slot: `{ scr = FlipTrick params = { Name score anim speed
  trickslack ... } }`. `FlipTrick` plays `anim` at `speed` times
  `Skater_Flip_Speed_Stat` (1.0-1.3, at most 1.3), names it after 15
  frames and turns `BailOn` until `trickslack` (10) frames from the end;
  then the next trick can start (`DoNextTrick`). `GrabTrick` plays its
  way in, names it halfway, comes out if the button is let go after 60%,
  and otherwise holds `idle` (tweaking) until it is, then plays `anim`
  backwards. Ported for the default controls, without spins, specials,
  extras, tweak points or sounds.
- **Scoring.** The scripts' `SetTrickName`, `SetTrickScore` and `Display`
  are skater commands (the dispatcher around 0x80112C04): `Display` hands
  the trick to the score object (0x800B05BC) with flags (`BlockSpin` 2,
  `NoDegrade` 0x40...), and the spin so far (skater `+0x3544`, degrees)
  to 0x800B0A00, which counts 180s as `(degrees + spin_count_slop) / 180`
  (slop 60) on the latest trick, never down. Each trick records how many
  times the same trick came before it in the combo. 0x800B124C totals
  them: `score * degrade[repeats] * spin / 200`, with degrade 100, 90, 80,
  70, 60, 50 (percent, 0x800B1658) and spin 2, 3, 4, 6, 8, 10 (halves,
  0x800B1630); the spin factor is set by the combo's first trick or the
  first after a `BlockSpin` one (grinds and manuals) and carries on to
  the tricks after it. The multiplier (`+0x58`) goes up one per trick;
  the combo scores the sum times it, and the same difference feeds the
  special meter (`+0x64`, capped at 3000). Tweaks (0x800B0BA0) add points
  straight to the latest trick each frame: the grind update adds the
  grind's `SetGrindTweak` (7, 36 for specials; stored at `+0x3AC4`), the
  balance update the trick's `DoBalanceTrick Tweak` (1 in a manual, 5 for
  specials), and a held grab its `GrabTweak` (`GRABTWEAK_MEDIUM`, 20).
  The special meter (score object `+0x64`, full flag `+0x68`): with
  `NewSpecial = 1` (PHYSICS.q) it gains whatever the combo's total gains
  as it's recomputed, fills at 3000 (`0xBB8`) and then allows specials;
  each frame (0x800AFCF0) it drains 50 a second, or 200 while full; a bail
  empties it. Specials are `TripleInOrder` combinations (two directions
  then a button within 400 ms): each character's slots come from the goal
  that unlocks them (`goal_get_special_trick_display_text`: Jessie's grab
  is `SpAir_D_R_Square`, her manual `SpMan_R_L_Triangle`, her lip
  `SpLip_L_D_Triangle`), filled by `Trick_<name>SpGrab`, `...SpManual`,
  `...SpLip`. Special grabs tweak 30 a frame (`GRABTWEAK_SPECIAL`),
  special manuals 5. Ported, with every special unlocked (the special lip
  too), without `NoDegrade`.
- **Lips.** State 3 (0x80103418) is the lip, not a wall ride. Grind start
  (0x80108470) turns a rail into a lip when the skater is rising, flat
  against the ramp (matrix up Y under `sin(LipPlayerHorizontalAngle)`,
  47 degrees), facing up, on a steep ramp (ground normal Y under
  `cos(LipRampVertAngle)`, 68.5) and not twisted (side Y under
  `sin(LipAllowAngle)`, 35, or `LipAllowAngle_Override`, 60, for flagged
  rail nodes): velocity zeroed, state 3, at the rail point, and the
  `LipTrick` script with the `Lip_Triangle` slot's params. `LipTrick`
  waits five frames for `SpecialLipTricks` (two directions then Triangle
  within 1000 ms) or `LipTricks` (`{ Press, Left, 500 }` and so on, slots
  `Lip_TriangleL`...), else goes on to `LipMacro2`: way-in animation,
  then balancing (`LipParams`, Right and Left, the range animation played
  backwards along the meter) with `TweakTrick 10` a frame. Ollying ends
  it with `LipOut` (`NoOllie` tricks) or `OllieLipOut`; off the meter one
  way `LipOut`, the other a spine transfer if the deck's skateable or
  `LipBail`. `LipOut` plays the way out and puts the skater back in the
  air a unit up and out, turned to drop back in. Ported: lips from vert
  air at coping rails (the angle tests approximated by vert air rising
  at a level rail), the trick choice, balance, tweaks and the ways off,
  without spine transfers (the unsafe side bails) and lip combos. On the
  Hub, every quarter pipe with a coping rail lips
  (`LIPS=1 cargo run --release -p desa_viewer --example vert_ramps`).
- **Animations and landing rules** (`TRICKS.q`). `OnGroundAI` plays
  `StandTurnLeft`/`Right` then their `...Idle` loops while steering
  (`CrouchTurn...` crouched), `CrouchIdle` crouched, and otherwise
  `StandIdle`, with `PushCycle1`/`2` while pushing (`just_coasting`).
  `GroundGone` plays `Stand2InAir`; `Airborne` holds `AirTurnLeft`/`Right`
  while steering, loops `AirIdle`, and stretches the legs
  (`StretchLegsInit`) with under 0.2 s of air left. `Land` bails a
  landing faster than 500 that's 60 to 120 degrees off the way the skater
  was going (`YawBail`), and pitch and roll bails; `Land2` plays
  `LandBackward1`/`2` landing backwards (fakie), `LandSketchy` with a
  "Sketchy" message 45 to 60 degrees off after 0.5 s in the air (0.75
  crouched), `LandSmall` or `CrouchBumpDown` after under 0.2 s, and
  `CrouchBumpDown` or `Land1`/`2` otherwise. Pushing needs the
  controller's AutoKick option (`+0x3A38`, set by `AutoKickOn`): on, the
  can-push test (0x800F43F0) passes under the kick speed with no button.
  A push, a landing, an ollie and a bail play through before anything
  less important takes over (the scripts wait on `AnimFinished`), and
  `DoAPush` runs a whole `PushCycle` before coasting. `GeneralBail` turns
  the skater to face its velocity (`TurnToFaceVelocity`) and
  `DoingTrickBail` falls forwards (`Bail1` or `Bail2`, at random) or,
  landing backwards, backwards (`BailBackward`), the bail lasting the
  fall and the get-up. Crouching runs `DoCrouch_slope`: `CrouchIdle`,
  or `CrouchBumpUp`/`Down` when the slope changes by more than 5 degrees
  in a frame (never the `Crouch` animation, which some characters have
  for another skeleton). Landing backwards, `Land2`'s `FlipAndRotate`
  turns the skater round to face the way it's going and flips its stance
  (`Flipped`), so it rides switch: the ground turn animations swap, and
  the game draws the skeleton mirrored through each bone's mirror
  partner (`.ske`). Every `PlayAnim` blends from the last pose over its
  `BlendPeriod`: 0.3 seconds usually, 0.1 into landings, 0.03 into
  flails, none into an ollie, a backwards landing or a bail's get-up.
  Ported: all of these (the skate crate's `anims` module picks them,
  with priorities, play-through and blend periods; the viewer blends and
  mirrors), AutoKick on by default, and the yaw bail; not the pitch and
  roll bails.
- **Trigger faces** (flag `0x40`) belong to level geometry whose node has
  a `TriggerScript`, run when the skater touches them. Teleporters end a
  few calls down in a restart named as `node =` (the Hub harbour's water
  runs `Object03c_DOIT`, `Teleporter_water node = TRG_WaterStart`); gaps
  run `StartGap GapID = X flags = [ CANCEL_GROUND ]` and `EndGap GapID = X
  text = "Chain Link Gap" score = 100`, and the gap scores as a trick in
  the combo if nothing its flags rule out happened in between
  (`CANCEL_GROUND`, `CANCEL_AIR`, `PURE_AIR`, `REQUIRE_RAIL`,
  `REQUIRE_LIP`). Ported: teleporters and gaps, the scripts read without
  running them (`desa_viewer::triggers`). Our own rules: gaps fire on
  crossing a trigger, ends before starts, and only after 50 units across
  (start and end pads lie together for gaps that go both ways). Not yet:
  the other trigger scripts (breakables, goals, sounds).
- **Grind and manual variety.** `GrindTricks` (`disneytricks.q`) picks
  the grind by the direction with Triangle (`AirTrickLogic`, 500 ms):
  none `Grind1`, up `Grind2`, right `Grind3`, down `Grind4`, left
  `Grind5` (`Grind1_180`... when turning 180 onto the rail, not ported).
  `ManualTricks` and `GroundManualTrickBranches` (`manualtricks.q`) start
  or branch to `Manual2` to `5` with Circle and a direction (400 ms);
  up-down is `Manual1`. Each trick names its in and range animations
  (`InitAnim`, `anim` or `BalanceAnim`), played along the balance meter.
  Ported.
- **Reverts.** Landing from vert, `Land2` sets the `Reverts` extra
  tricks (`{ Press, R2, 200 }` and `L2`, slots `ExtraSlot1`/`2`, both
  `Trick_Revert`) for `RevertTime = 5`. `Revert` scores 100 (`FS Revert`
  or `BS Revert` by the flags or the last spin's way), flips the skater
  round (`FlipAfter`), and lets a manual carry the combo on while its
  animation plays. Ported: R within 200 ms before landing or 5 frames
  after; the combo lands about 0.6 s later without a manual.
- **The main update** (0x8010B120) runs the speed limits (0x800F4834)
  every frame before handing over to the state's update: ground (state
  0, 0x800FB3E4), air (1, 0x800FC7F8), vert (2, 0x800FE664), 3
  (0x80103418, wall riding perhaps) and rail (4, 0x80100A70). So the cap
  at `Skater_Max_Max_Speed` holds in the air and on rails too, which is
  what keeps chained grinds (each adding `Rail_Speed_Boost`) from
  building speed without end. Then it tries a grind (0x801078A8, with 0:
  only from the air). `Skater_default_head_height` isn't read by the
  physics: the knee-height line is the only check ahead.
- **Falling through the ground (fixed).** Two ways the port let the
  skater through: an ollie from flat ground started exactly on the floor,
  so the first air step's line could catch the floor from below and the
  wall response pushed the skater 9 units through it (the skater now
  takes off a unit up, as the game lands it a unit up); and rolling
  backwards into the foot of a slope, the facing-based stick test read the
  flat as ground falling away and launched the skater from just under it
  (ground above where the skater is going now always sticks).
  `cargo run --release -p desa_viewer --example skate_check` skates a
  level at random and reports any frame through the ground, and how often
  rail approaches grind. The grind button now looks for a rail for half a
  second after it's pressed (`{ Press, Triangle, 500 }`), as well as while
  held; falling out of the level puts the skater on the ground at the
  nearest spawn, as soon as it's 1000 below where it last stood with
  nothing underneath.
- **Rails inside ledges.** Many rails run a few units below the top edge
  of the ledge or kerb they follow, so a skater leaving one starts just
  inside it, where the port's two-sided ray casts find the ledge's
  inside faces. The port lifts a skater leaving a rail onto any ground
  within `Physics_Ground_Snap_Up` above it, and puts one that falls 500
  below the level's collision back where it last stood (the game has its
  own out-of-bounds triggers). Dropped onto every rail of every level
  both ways (`cargo run --release -p desa_viewer --example rail_drops`),
  about 27 of 27,000 still fall out, at ledge edges over pits and drops
  and at rails out of reach below the levels. How the game avoids this
  (one-sided collision, perhaps) isn't known yet.

## Next

Read the rest of the on-ground update and its helpers, then the air
update, and check the port frame by frame against Dolphin (its debugger
can break on these addresses and show the skater's velocity at
`[[skater]+0x698]+0x34`).
