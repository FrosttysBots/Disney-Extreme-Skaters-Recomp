# extreme-skate-rs

A Rust reimplementation of *Disney's Extreme Skate Adventure* (Toys for Bob, 2003),
built by studying the GameCube release. It runs on Neversoft's THPS4-era engine.

This repository contains **only original code**. To play or develop, you
supply your own disc image; game assets are never committed (see `.gitignore`).

## Map Viewer

`DESA Map Viewer.exe` lets you fly around every level, straight from your
disc image; nothing needs unpacking first. Any of the 12 playable
characters can stand in the level on their board and play their
animations.

```
cargo build --release -p desa_viewer
```

This builds `target/release/desa-map-viewer.exe`, which needs no installs.
On first run it looks for the disc image (`.iso`) next to itself and in the
folders above it; otherwise use **Open disc image...** in the panel. It
remembers the disc, the last level, the character and the camera speed in
`%APPDATA%\desa-map-viewer\settings.txt`.

The side panel lists the 11 levels and has toggles for the sky, rails
(magenta tubes), spawn points (green = player 1 start, blue = others) and
collision, a brighten slider for dark areas, camera speed, and every
spawn point by name to jump to.

Under **Character**, pick a character and one of their 125-133
animations, then play, pause, change the speed or drag the time slider.
The character stands on the level's start, and moves to each spawn point
you go to (the spawn markers are switched off then, since they'd stand
right through the character). **Look at the character** puts the camera in
front of them, and **Blink** makes them blink every few seconds (see
[Blinking](#blinking)); Tantor, Tarzan, Woody and Zurg have no blink
frames.

Under **Camera paths**, each level lists its cutscene, goal and warp
cameras (7 to 46 per level). Click one to watch it; moving or looking
around takes over from wherever it is.

| Control | Action |
|---|---|
| Hold right mouse | Look around |
| W A S D | Move |
| E / Space, Q / Ctrl | Up, down |
| Shift | Move 5x faster |
| Mouse wheel | Change speed |
| Tab | Next spawn point |
| K | Cycle collision view |
| R | Back to the start |
| P | Play or pause the character |
| [ and ] | Previous or next animation |
| F1 | Hide or show the panel |

`desa-map-viewer --level HUB --screenshot out.png` renders one frame,
panel included, without opening a window. Add
`--character jessie --animation Ollie --time 0.4` to show a character
there, with the camera looking at them, or `--camera-path Cam_Plane
--time 8` to view from a camera path.

## Crates

| Crate | What it does |
|---|---|
| `gc_disc` | Reads GameCube disc images: header, file system table, `main.dol` sections |
| `prg` | Reads the `.prg` archives (Neversoft PRE format, LZSS-compressed) that hold almost all game data |
| `ngc_texture` | Decodes `.img.ngc` images and `.tex.ngc` texture dictionaries (GX CMPR and RGBA8) |
| `ngc_model` | Parses `.mdl.ngc` models, `.skin.ngc` skinned characters and `.scn.ngc` level scenes: materials, vertex arrays, bone weights, triangle strips |
| `ngc_anim` | Parses `.ske` skeletons and `.ska.ngc` animations, samples them and poses skinned characters |
| `ngc_collision` | Parses `.col.ngc` collision meshes, repairing the counts the original tool corrupted |
| `qb` | Tokenizes, decompiles and parses Neversoft QB scripts (level node arrays, game logic) |
| `desa_viewer` | Level and character rendering (wgpu), plus `desa-map-viewer` (the Map Viewer) and `desa-viewer` (command-line viewer and screenshots) |
| `desa_cli` | The `desa` command-line tool built on top of them |

## Usage

The image must be a plain `.iso` / `.gcm`. If yours is `.rvz`, `.gcz`, `.wia`
or `.ciso`, convert it in Dolphin first: right-click the game, choose
**Convert File...**, and set **Format** to **ISO**.

```
cargo run --release -- info  game.iso            # header, region, file counts
cargo run --release -- ls    game.iso            # every file with its size
cargo run --release -- dol   game.iso            # main.dol load addresses (for Ghidra)
cargo run --release -- extract game.iso extracted
cargo run --release -- extract game.iso extracted --prefix levels
```

`extract` uses the same layout as Dolphin's "Extract Entire Disc": `sys/`
holds `boot.bin`, `bi2.bin`, `apploader.img`, `main.dol` and `fst.bin`, and
`files/` holds the game's file system.

Then unpack the archives in `files/pre/`:

```
cargo run --release -- prg ls extracted/files/pre/qb.prg
cargo run --release -- prg unpack extracted/files/pre extracted/unpacked
```

Each archive unpacks into its own folder, e.g. `extracted/unpacked/qb/scripts/*.qb`.
A few archives list the same sound twice with different capitalization; the
copies are identical, so on Windows one simply overwrites the other.

To check the `prg` crate against every archive on the real disc:

```
cargo test --release -p prg -- --ignored
```

Export images and textures as PNGs (a file, or everything under a folder):

```
cargo run --release -- tex export extracted/files/images extracted/textures/images
cargo run --release -- tex export extracted/unpacked extracted/textures/unpacked
cargo run --release -- tex ls extracted/unpacked/beachScn/Levels/beach/beach.tex.ngc
```

An `.img.ngc` becomes one PNG. A `.tex.ngc` becomes a folder of PNGs named
by each texture's checksum, which is how models refer to them. Only the
full-size mip level is exported.

Export a model or a whole level as OBJ + MTL + PNG textures, ready to open
in Blender. Textures come from the `.tex.ngc` with the same name next to the
input, or from `--textures`:

```
cargo run --release -- model info   extracted/unpacked/beachScn/Levels/beach/beach.scn.ngc
cargo run --release -- model export extracted/unpacked/beachScn/Levels/beach/beach.scn.ngc extracted/models/beach
cargo test --release -p ngc_model -- --ignored    # parse every real model and level
```

Vertex colors (the levels' baked lighting) are written as OBJ vertex colors.
`tools/render_obj.py` and `tools/render_textured.py` render quick previews of
an exported OBJ with Python and Pillow, for checking results without Blender.

Inspect an animation, or export a character posed by one at a given time
(in seconds). The skeleton, the key tables and the rest pose are found
automatically when everything is unpacked under one folder; `--skeleton`,
`--tables` and `--rest` override them:

```
cargo run --release -- anim info extracted/unpacked/anims_jessie/anims/jessie/Ollie.ska.ngc
cargo run --release -- anim pose extracted/unpacked/anims_jessie/models/jessie/jessie.skin.ngc extracted/unpacked/anims_jessie/anims/jessie/Ollie.ska.ngc extracted/models/jessie_ollie --time 0.4
cargo test --release -p ngc_anim -- --ignored    # parse every real skeleton and animation
```

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
set, scrolling UVs and environment mapping. Not drawn yet: vertex-color
animation, fog, and the objects placed by scripts (pedestrians, goal items).
`desa model materials <file>` lists a level's materials by area covered,
which helps track down rendering problems.

The level's collision is found automatically (`<X>col/Levels/<name>/` next to
the `<X>Scn` folder) and shown with **K**, or `--show-collision overlay|only`
for screenshots. Colors: yellow = trigger or non-collidable, red = vert
(quarter pipes), blue = wall-ridable, purple = not skatable, gray = the rest.

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

## Collision format

```
cargo run --release -- col info   extracted/unpacked/HUBcol/Levels/HUB/HUB.col.ngc
cargo run --release -- col export extracted/unpacked/HUBcol/Levels/HUB/HUB.col.ngc extracted/models/HUB_col.obj
cargo test --release -p ngc_collision -- --ignored    # parse every real collision file
```

The layout (in the module docs of `crates/ngc_collision`) matches the rest
of Neversoft's THPS engine family: object records with bounding boxes,
float vertices, a brightness byte per vertex, faces with 8- or 16-bit
indices, and a BSP tree (kept as raw bytes for now). Each face has flags
and a terrain type (which picks sounds and particles). Objects share their
checksums with the level's visible sectors; many are collision-only, such
as invisible walls and trigger volumes. Rails aren't collision faces: in
this engine they're path nodes defined by the level scripts.

**Corrupted counts.** The tool that wrote these files turned `0x20` bytes
into `0x00` in some count and offset fields (32 reads as 0, 288 as 256,
1056 as 1024). This is the same bug behind the textures' "0 means 32". The
parser rebuilds each object's true counts by picking the reading that keeps
all offsets and totals consistent while assuming the fewest corrupted
fields. On the US disc it repairs 88 counts across 271 files, and all
457,940 faces load.

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
  parent), each vertex with two weights, plus a small second block of what
  look like extra influences (kept raw for now). Positions are in model
  space in the rest pose, so characters export and draw without a
  skeleton. Some group counts suffer the same `0x20` -> `0x00` corruption as
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

## Roadmap

1. **Asset tools.** Disc reading, PRG unpacking, textures, static and
   skinned models, levels, collision, a level viewer, skeletons and
   animations, camera paths, and animated characters in the Map Viewer
   (done). Next: the skins' extra-influence block.
2. **Level loading.** Rails and spawn points (done). Next: objects and
   pedestrians from the node arrays, the collision BSP tree, fog and
   vertex-color animation.
3. **QB scripts.** Decompiling and data parsing (done). Next: an
   interpreter that runs the game's scripts.
4. **Skater physics.** Match the original's constants and update loop,
   verified against Dolphin frame by frame.
5. **Gameplay.** Tricks, scoring, goals, game modes, UI and audio.

## Reverse-engineering setup

- Load `extracted/sys/main.dol` into Ghidra with the
  [GameCube loader](https://github.com/Cuyler36/Ghidra-GameCube-Loader).
- `desa dol` prints the section addresses for checking the memory map.
- The US disc (`GEXE52`) ships no `.map` symbol file, so Ghidra's function
  names will be auto-generated.
- The PC release of THPS4 runs on the same engine family and is x86, which
  is much easier to read when you're working out shared systems.
