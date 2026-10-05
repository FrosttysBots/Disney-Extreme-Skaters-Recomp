# extreme-skate-rs

A Rust reimplementation of *Disney's Extreme Skate Adventure* (Toys for Bob, 2003),
built by studying the GameCube release. It runs on Neversoft's THPS4-era engine.

This repository contains **only original code**. To play or develop, you
supply your own disc image; game assets are never committed (see `.gitignore`).

## Crates

| Crate | What it does |
|---|---|
| `gc_disc` | Reads GameCube disc images: header, file system table, `main.dol` sections |
| `prg` | Reads the `.prg` archives (Neversoft PRE format, LZSS-compressed) that hold almost all game data |
| `ngc_texture` | Decodes `.img.ngc` images and `.tex.ngc` texture dictionaries (GX CMPR and RGBA8) |
| `ngc_model` | Parses `.mdl.ngc` models and `.scn.ngc` level scenes: materials, vertex arrays, triangle strips |
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
- **Not supported yet:** skinned characters (`.skin.ngc`), which group
  vertices by bone, and collision (`.col.ngc`).

Result on the US disc: all 195 models and 22 level scenes (11 levels plus
their skies) parse exactly to the end of the file.

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
| `.ska.ngc` | 2489 | Skeletal animations |
| `.dsp` | 736 | GameCube ADPCM sound effects |
| `.img.ngc` / `.tex.ngc` | 679 / 408 | Images and texture dictionaries |
| `.qb` | 347 | Compiled QB scripts |
| `.col.ngc` | 271 | Collision meshes |
| `.mdl.ngc` / `.skin.ngc` / `.scn.ngc` | 195 / 191 / 22 | Models, skinned characters, level scenes |
| `.cas.ngc` | 191 | Create-a-skater parts |
| `.ske` | 53 | Skeletons |
| `.fnt.ngc` | 32 | Fonts |

## Roadmap

1. **Asset tools.** Disc reading, PRG unpacking, textures, and static
   models and levels (done). Next: skinned models, collision, and a level
   viewer.
2. **Level loading.** Render whole levels with collision and a free camera.
3. **QB scripts.** Parse and run Neversoft's compiled script format.
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
