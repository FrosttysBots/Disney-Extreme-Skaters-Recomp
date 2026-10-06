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
  be read, steering (0x800ED848), speed limits and friction among them.

## Next

Read the rest of the on-ground update and its helpers, then the air
update, and check the port frame by frame against Dolphin (its debugger
can break on these addresses and show the skater's velocity at
`[[skater]+0x698]+0x34`).
