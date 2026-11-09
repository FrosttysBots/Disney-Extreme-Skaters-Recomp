# Formats, physics and reverse engineering

How the game's files and code were worked out. Back to the [README](../README.md).

Every format is checked against every file on the US disc (`GEXE52`):
69 archives (5,616 files), 3,364 textures and 48 loading screens, 408
models, characters and levels, 271 collision files (458,036 faces, 110,174
BSP nodes), 347 scripts, 53 skeletons, 2,197 animations and 292 camera
paths all load. The original export tool corrupted many count and offset
fields (it turned `0x20` bytes into `0x00`); the parsers repair them, and
each section below says how.

## Level viewer

Fly around any level with its sky, textures and baked lighting:

```
cargo run --release -p desa_viewer -- extracted/unpacked/HUBScn/Levels/HUB/HUB.scn.ngc
```

| Control | Action |
|---|---|
| Hold right mouse | Look around |
| W A S D | Move |
| E / Space, Q / Ctrl | Up, down |
| Shift | Move 5x faster |
| Mouse wheel | Change speed |
| K | Cycle collision: hidden, overlay, collision only |
| C | Print the camera as a `--camera` value |
| R / Esc | Reset the camera / quit |

The sky is found automatically (`<name>_sky/<name>_sky.scn.ngc` next to the
level's folder); `--no-sky` or `--sky` override it. To render one frame to a
PNG without opening a window, which is handy for comparing changes:

```
cargo run --release -p desa_viewer -- extracted/unpacked/HUBScn/Levels/HUB/HUB.scn.ngc --camera=-1400,900,4000,40,-8 --screenshot hub.png
```

It also draws the level's rails and spawn points from its node array
(`<X>/levels/<name>/<name>.qb`), and starts the camera at the player 1
spawn; `--no-nodes`, `--hide-rails` and `--hide-spawns` turn them off.
Node positions use the opposite Z direction from the meshes: mirroring Z
puts 96% of the hub's rail nodes on a collision surface, against 0% as
stored. Spawn headings follow the engine's +Z-forward convention mirrored
the same way; the spawn views look right, but that convention hasn't been
checked against the running game.

Every material pass is drawn with its blend mode (opaque, add, subtract,
alpha blend, modulate, brighten, and their fixed-alpha variants), its own UV
set, scrolling UVs and environment mapping. This viewer draws every
sector; the Map Viewer leaves out the ones that aren't there at the start,
adds objects and animates vertex colors. Neither draws fog, which the
GameCube executable seems to leave out too: its `SetFogColor` prints
"STUBBED" in dozens of places.

**Animated vertex colors.** A material pass can carry color sequences
(times in milliseconds, looping at the last key, colors where 0x80 is full
brightness), and a sector can give each vertex a sequence number: 0 for
none, otherwise 1-based into the sequences of the material drawing it. The
sequence's color replaces the baked one. Only three levels use it, for 803
vertices: the arcade screens at the pizza level flicker on three times a
second, and there are smaller effects on the hub (beside each warp portal)
and the beach.
`desa model materials <file>` lists a level's materials by area covered,
which helps track down rendering problems.

The level's collision is found automatically (`<X>col/Levels/<name>/` next to
the `<X>Scn` folder) and shown with **K**, or `--show-collision overlay|only`
for screenshots. Colors: yellow = trigger or non-collidable, red = vert
(quarter pipes), blue = wall-ridable, purple = not skatable, gray = the rest.

### Objects and pedestrians

Besides rails and spawns, a level's node array places objects:

- **`LevelGeometry` and `LevelObject`** nodes name sectors of the level
  scene. Those without the `CreatedAtStart` flag aren't there when the
  level starts: trigger boxes, goal pickups, warp portals that open later
  (298 sectors on the hub).
- **`GameObject` and `Vehicle`** nodes place a model, `Models/<path>.ngc`
  in the level's archive (`X.prg`).
- **`Pedestrian`** nodes place a skinned model from `XPed.prg` with a
  `SkeletonName` and an `AnimName`: a script in `scripts/allanims.qb`
  whose `LoadAnim` lines give each animation a role. The viewer plays the
  `Ped_Guide_Idle1` or `Ped_M_Idle1` one. Some sets borrow animations
  (birds use Zazu's; Tarzan's Buzz uses Buzz's from `anims_buzz.prg`).

While skating, the objects' scripts see the skater: birds, bats and crows
take off when it comes within their `Obj_SetInnerRadius` (the scripts'
`SkaterInRadius` exceptions), flying to their next perch or away, with
their wing-flap and squawk sounds; objects a script creates (the Hub's
flying birds) appear and run their own scripts. Other pedestrians (Scar,
hyenas, wildebeest) are solid: the skater bumps off them and flails, as
off a wall.

All 1,795 object nodes on the disc load. Objects are turned by `-heading`
about Y, which is what mirroring Z does to a rotation; that's the opposite
way round from the spawn convention above, and goal intro cameras don't say
which is right (they see their pedestrian from the front 19 times out of
40 this way).

## Scripts (QB)

Most of the game's logic and every level's object placement live in compiled
QB scripts: the 188 global scripts in `qb.prg`, plus each level's
`<level>.qb` (its `NodeArray`: rails, spawn points, objects, pedestrians)
and `<level>_scripts.qb`.

```
cargo run --release -- qb decompile extracted/unpacked extracted/scripts
cargo run --release -- qb nodes extracted/unpacked/beach/levels/beach/beach.qb
cargo test --release -p qb -- --ignored    # tokenize, parse and decompile every script
```

`decompile` writes readable `.q` source next to the original paths, for
example `extracted/scripts/beach/levels/beach/beach.q`. `nodes` counts a
level's nodes by class and its linked rail nodes.

- **Tokens** follow the compiler shared across the THPS series: one-byte
  codes, some followed by data. Numbers are **little-endian**, unlike the
  rest of the GameCube data.
- **Names** are stored as checksums: a CRC-32 of the lowercased name
  without the final inversion. Each file ends with a symbol table giving
  the original names, so the decompiled scripts use the developers' own
  names. Across the disc there are 41,018 names, and every checksum used in
  every script is covered. Names that aren't plain identifiers print as
  `#"Big 1"`; unknown ones would print as `#0x1234abcd`.
- **`Random(@a @b ...)`** is stored as a table of offsets with jumps between
  the choices; the decompiler rebuilds the original form.
- **Data definitions** such as `NodeArray` parse into structured values
  (`qb::Value`), ready for the viewer and gameplay code.

On the US disc all 347 scripts tokenize to their last byte and parse:
7,046 scripts and 25,503 level nodes.

**Running scripts.** `qb::vm` is the start of an interpreter: a `Program`
holds every script and global value, and a `Thread` runs one script for
one object a few statements at a time, stopping when it waits, so many
objects run side by side. It handles calls with parameters (`<name>`,
`<...>`), assignments, `if`/`elseif`/`else` with `NOT`, `begin ... repeat
n`, `break`, `return`, `wait n [seconds]`, `object:command` and simple
arithmetic; anything that isn't a script goes to the game as a command.
The Map Viewer runs every starting object's `TriggerScript` this way and
implements path following (`Obj_FollowPathLinked`, path velocity,
acceleration, random forks), `Obj_PlayAnim`, `Obj_MoveToNode`, waiting for
animations and moves, `create`/`kill`/`Die` and `IsAlive`, which sets 29
objects moving across the levels (the Hub's airplane and crane among
them). The rest (event handlers, trigger radii, sounds, flags) does
nothing yet: most of it reacts to the skater. Speeds take a unit as an
inch (1 mph = 17.6 units a second), which hasn't been checked against the
game.

## Collision format

```
cargo run --release -- col info   extracted/unpacked/HUBcol/Levels/HUB/HUB.col.ngc
cargo run --release -- col export extracted/unpacked/HUBcol/Levels/HUB/HUB.col.ngc extracted/models/HUB_col.obj
cargo test --release -p ngc_collision -- --ignored    # parse every real collision file
```

The layout (in the module docs of `crates/ngc_collision`) matches the rest
of Neversoft's THPS engine family: object records with bounding boxes,
float vertices, a brightness byte per vertex, faces with 8- or 16-bit
indices, and a BSP tree per object. Each face has flags
and a terrain type (which picks sounds and particles). Objects share their
checksums with the level's visible sectors; many are collision-only, such
as invisible walls and trigger volumes. Rails aren't collision faces: in
this engine they're path nodes defined by the level scripts.

**Corrupted counts.** The tool that wrote these files turned `0x20` bytes
into `0x00` in some count and offset fields (32 reads as 0, 288 as 256,
1056 as 1024). This is the same bug behind the textures' "0 means 32". The
parser rebuilds each object's true counts by picking the reading that keeps
all offsets and totals consistent while assuming the fewest corrupted
fields. When more than one reading fits, the BSP trees decide, since they
only read cleanly where the faces really end: that fixed Hamm, whose 544
vertices were stored (with the header's total) as 512, which had shifted
every face. On the US disc it repairs 90 counts across 271 files, and all
458,036 faces load.

**BSP trees.** Each object's tree splits space on x, y or z; a split's
lower child comes right after it and its upper child after the lower's
whole subtree, and each leaf lists the faces touching its box, as indices
into one shared face list after all the nodes. The same `0x20` bug hits
child offsets, leaf face counts and start indices, and the objects' root
offsets, so the reader takes the tree's shape as the truth, checks the
stored offsets against it, and solves the leaves' ranges together (a leaf
of 32 faces otherwise reads as empty). It repairs 364 fields, reads all
110,174 nodes, and `BspTree::faces_near` finds 99.94% of the faces a
brute-force search finds near a point; the rest are listed by the game in
a leaf whose box they don't touch.

## Model formats

`.mdl.ngc` (objects) and `.scn.ngc` (levels) share one big-endian layout; the
exact field list is in the module docs of `crates/ngc_model`.

- **Materials** have one or more passes. Each pass names a texture by
  checksum, plus blend settings, a color, and optional UV-scrolling and
  vertex-color animation blocks, depending on its flags.
- **Sectors** are objects: a vertex pool (positions, 16-bit normals,
  interleaved UV sets, colors where 0x80 is full brightness) plus meshes.
- **Meshes** draw triangle strips from the pool with one material. Strips
  wind counter-clockwise.
- **Coordinates** are Y-up. UVs use a bottom-left origin, which matches the
  bottom-up texture storage, so they export to OBJ unchanged.
- **Skinned characters** (`.skin.ngc`) use the same layout, except that
  their vertices come in groups sharing a pair of bones (a bone and its
  parent), each vertex with two weights. A second block gives 3,701
  vertices on the disc a third bone with a small weight (1/21 to 1/3), the
  two main weights then adding up to one minus it; posing with it
  stretches the mesh less for 9 of the 11 characters that have any. It
  also holds copies of those vertices, which undo 40 values that the
  `0x20` bug damaged in the main data (10.0 stored as 8.0). Positions are
  in model space in the rest pose, so characters export and draw without
  a skeleton. Some group counts suffer the same `0x20` -> `0x00` corruption as
  collision; the parser repairs them by keeping the reading where every
  group header lines up.

```
cargo run --release -- model export extracted/unpacked/anims_jessie/models/jessie/jessie.skin.ngc extracted/models/jessie
```

Result on the US disc: all 195 models, 22 level scenes (11 levels plus their
skies) and 191 skinned models parse exactly to the end of the file.

## Skeletons and animations

Both formats are documented in the module docs of `crates/ngc_anim`. The
decoder was checked against the game's own (at `0x80067BE8` in `main.dol`).

- **Skeletons** (`.ske`, little-endian) are just names: for each bone its
  checksum, its parent's and its mirror partner's (left arm <-> right arm).
  There is no rest pose in them.
- **Animations** (`.ska.ngc`) have a big-endian header and per-bone key
  counts, then little-endian key streams. Keys are at 60 frames per second
  and relative to the parent bone.
- **Rotation keys** are compressed quaternions: x, y and z as 16-bit values
  (or unsigned bytes when small) scaled by 1/16384, with w rebuilt from them
  and its sign in a flag bit. A key can instead be a 1-byte index into
  `standardkeyq.bin`, a table of common rotations loaded at start-up.
  Translations work the same way, divided by 32, with `standardkeyt.bin`.
- **Posing.** The decoded quaternions must be conjugated. A bone's matrix
  is parent * translate * rotate, and a vertex follows its bones by
  pose * inverse(rest), where the rest pose is the character's
  `default.ska.ngc` at time 0. The two weights of each skinned vertex are
  stored in reverse bone order. Getting any of these wrong tears the mesh
  apart at the joints.
- **Camera paths** (header flags `0x1E000000`) are the same file type
  with plain big-endian floats: one track of rotation and translation
  keys, plus "custom" keys that set the field of view. The camera looks
  down its local -Z after the same conjugation as bones; with that, the
  view rays of the 93 moving paths meet at their subject (median error
  0.4 degrees, against 50 or more for any other choice). Positions are in
  mesh space, unlike node positions. Frame numbers have the `0x20` ->
  `0x00` corruption and are repaired by keeping them in order. Many paths
  hold their last key for much longer than they move (the hub's warp
  shots say 55.9 seconds but stop at 11), leaving it to the scripts to
  cut away. Details are in `crates/ngc_anim/src/camera.rs`.

Result on the US disc: all 53 skeletons, 2,197 bone animations and 292
camera paths parse.

### Blinking

Most characters' eyes are a small mesh textured with `eyes.png`. The same
model carries three more frames on tiny quads hidden inside the body:
`eyes00` (open), `eyes01` (half shut) and `eyes03` (shut). The names come
from the created-skater script `disneytricks.qb`, which swaps all four
together, and the texture checksums are the same in every character.

The GameCube executable never refers to these textures: there is no
checksum, name or format string for them, and
`CSkinComponent::replace_texture` only prints "STUB FUNCTION called". So
the GameCube release may not blink at all. The Map Viewer plays the frames
anyway (half shut, shut, half shut, about 1/6 second in all, every 1.5 to
5.5 seconds) on a timing of our own; untick **Blink** to turn it off.

## Texture formats

Both files are big-endian; see the module docs in `crates/ngc_texture` for
the exact layouts.

- **Pixel formats:** CMPR (the GameCube's DXT1), CMPR plus a second CMPR
  texture whose green channel is the alpha, and tiled RGBA8 (loading screens
  and two level textures).
- **Rows are stored bottom to top.** The decoder flips them. When a picture
  is padded to a larger stored size, the padding comes first in stored order.
- **A stored 0 means 32.** The original tool wrote any width, height or size
  of exactly 32 (and a size of 8192) as 0.
- **Not supported yet:** the 8 memory-card icons, which use a palette format.

Result on the US disc: 48 loading screens plus 3,364 images and textures
from 1,079 files.

## Formats found so far

| Extension | Count | Probably |
|---|---|---|
| `.ska.ngc` | 2489 | Animations: 2,197 skeletal and 292 camera paths (decoded) |
| `.dsp` | 736 | GameCube ADPCM sound effects |
| `.img.ngc` / `.tex.ngc` | 679 / 408 | Images and texture dictionaries |
| `.qb` | 347 | Compiled QB scripts |
| `.col.ngc` | 271 | Collision meshes (decoded) |
| `.mdl.ngc` / `.skin.ngc` / `.scn.ngc` | 195 / 191 / 22 | Models, skinned characters, level scenes |
| `.cas.ngc` | 191 | Create-a-skater parts |
| `.ske` | 53 | Skeletons (decoded) |
| `.fnt.ngc` | 32 | Fonts |

## Skater physics

The game keeps its physics tuning in scripts: `PHYSICS.q` holds about 200
constants in the game's units (which look like inches): kick speeds of
355-496 a second (675 crouched), a top speed of 700-900, ollies at 400-450
up, air gravity of -1350, ground snapping 13 up and 8.2 down, and the
chase cameras. Many are `{ (min, max) STATS_X }` pairs scaled by one of a
character's stats, which are in their profiles in `disneytricks.q`
(Woody: `Air = 11`, `spin = 4`, ...); a stat of `s` gives
`min + (max - min) * s / 10`.

The `skate` crate reads them and runs a skater over a level's collision,
using the BSP trees for its ray casts. Its update follows the game's own
code in `main.dol`, found through the constants: the engine reads each
one by its name (or its name's checksum), and those lead straight to the
physics functions. Ported so far: pushing and drag, gravity along the
ground, steering, braking, the speed limits, the ollie (stronger the
longer the crouch), following the ground (and flying off the lip of a
kicker), bouncing off walls on the ground and in the air, the air step,
landing (including popping onto ledges), grinding level rails (rail
gravity, corners, jumping off), vert air off quarter pipes (coming back
down onto the ramp), and the balance meter for manuals and grinds (with
bails). Air tricks come from the game's scripts: each character's trick
table (Jessie's "Quickest Boots in the West", "Well Howdy There"...) and the
button combinations, played as the scripts play them, with a bail for
landing mid-trick. On the Hub, Jessie reaches her kick speed
of 425 a second, covers about 1,000 units in three seconds, ollies 57
units high and grinds the Hub's long rails.

[docs/physics-notes.md](physics-notes.md) maps every function read,
with addresses. Combos score as the game does: repeats degrade (100% down
to 50%) in a 2 minute run or a goal, as in the game's single session and
career (free skating doesn't degrade them, and its score is the best
combo rather than their sum, as the game's free skate has it), 180s multiply (x1.5 up to x5), grinds, manuals and held grabs
earn points every frame, and the total is the points times the number of
tricks. The special meter fills with points; full, each character's
special grab and manual (Jessie: down, right, flip for "Sit a Spell"; in a
manual right, left, grind for "Happy Trails") are on until it drains.
The skater pushes by itself (the game's AutoKick option, on by default:
untick it to push with W), turns, crouches, flies and lands with the
game's own animation choices, and bails landing sideways at speed. The
chase camera uses the game's camera settings, stays square to the ramp in
vert air, and pulls in rather than ending up behind a wall.
Lip tricks stall on the coping of quarter pipes with a rail along it:
press E (with a direction for other lips) as you launch off the lip, then
balance with A and D. Holding R going up a quarter pipe spine-transfers
to the ramp behind. Gaps score from the levels' trigger scripts, and
teleporters (like the Hub's harbour water) put the skater back. Moving
level pieces carry their collision with them as their scripts move them.
Not yet: a frame-by-frame check against Dolphin.
Turning rates are radians a second (the steering code turns by the rate
times the frame time), and the camera distances are feet (the camera
code multiplies them by 12).

## Roadmap

1. **Asset tools.** Disc reading, PRG unpacking, textures, static and
   skinned models, levels, collision, a level viewer, skeletons and
   animations, camera paths, and animated characters in the Map Viewer
   (done).
2. **Level loading.** Rails, spawn points, objects, pedestrians and
   animated vertex colors, the collision BSP trees (done).
3. **QB scripts.** Decompiling, data parsing, and an interpreter that
   runs objects' scripts, with path following and animations (done).
   Next: more commands, events (exceptions) and goals, which need a
   skater.
4. **Skater physics.** The game's constants, and its update code from
   `main.dol` for the ground, walls, air, landing, grinding, vert air,
   manuals, lips and balance (done; wall rides are unused in this game).
   Spine transfers too. Next: a frame-by-frame check against Dolphin.
5. **Tricks and scoring.** Each character's air tricks, grinds, manuals,
   lips and specials from the trick tables, reverts, the game's combo
   scoring and special meter, its animation choices, a chase camera and
   gamepad controls: skating is at an alpha (done; see
   [docs/alpha-testing.md](alpha-testing.md)).
6. **Gameplay.** Goals, game modes, the front end, UI and audio.

## Reverse-engineering setup

- Load `extracted/sys/main.dol` into Ghidra with the
  [GameCube loader](https://github.com/Cuyler36/Ghidra-GameCube-Loader).
- `desa dol` prints the section addresses for checking the memory map.
- The US disc (`GEXE52`) ships no `.map` symbol file, so Ghidra's function
  names will be auto-generated.
- The PC release of THPS4 runs on the same engine family and is x86, which
  is much easier to read when you're working out shared systems.
