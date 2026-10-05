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

1. **Asset tools.** Disc reading and PRG unpacking (done), then texture,
   model and level format decoders, ending in a level viewer.
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
