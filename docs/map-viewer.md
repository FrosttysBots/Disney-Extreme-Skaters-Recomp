# Map Viewer, game and tools

The details behind the [README](../README.md): the Map Viewer and the game, their controls and modes, screenshots from the command line, the `desa` tool and the crates.

## The viewer

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

The same build makes `target/release/desa-game.exe`: the game on its own,
without the viewer's panel, debug keys, rails, spawn markers or map. It
plays the opening movies, then comes up in the main menu (Esc pauses
while skating). It shares the viewer's settings, so a disc opened in
either is found by both; with no disc found it shows the panel to open
one.

The side panel lists the 11 levels and has toggles for the sky, rails
(magenta tubes), spawn points (green = player 1 start, blue = others) and
collision, a brighten slider for dark areas, camera speed, and every
spawn point by name to jump to.

Levels are shown as they start, with every object and pedestrian the node
array places there: goal pickups, vehicles and about 300 pedestrians, from
Scar and Kerchak to hyena tourists and birds, each playing its idle
animation. Objects run their own scripts, so vehicles like the Hub's
airplane follow their paths (see [Scripts](formats.md#scripts-qb)). **Goal objects** adds what goals and scripts create later
(the S-K-A-T-E letters, goal pedestrians, warp portals, pickups); see
[Objects and pedestrians](formats.md#objects-and-pedestrians).

Under **Character**, pick a character and one of their 125-133
animations, then play, pause, change the speed or drag the time slider.
The character stands on the level's start, and moves to each spawn point
you go to (the spawn markers are switched off then, since they'd stand
right through the character). **Look at the character** puts the camera in
front of them, and **Blink** makes them blink every few seconds (see
[Blinking](formats.md#blinking)); Tantor, Tarzan, Woody and Zurg have no blink
frames.

**Skate** puts the character on the level (an alpha: see
[docs/alpha-testing.md](alpha-testing.md) for how to test it and the
full controls): W pushes, S brakes, A/D steer,
holding Space crouches and letting go ollies, E grinds (ollie at a rail
holding E: like the game, grinds start in the air, with the button held;
with the simplified controls a press within half a second does),
tapping W then S starts a manual, Q flips and F grabs in the air (with a
direction: W, S, A, D), and Esc stops. Manuals balance with W and S, grinds
with A and D, on a meter at the bottom of the screen; the combo and the
score show above it and top right. The physics is the game's own code as far as it's been read
(see [Skater physics](formats.md#skater-physics)), with its constants and the
character's stats, and a chase camera set like the game's.

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
| W / S, A / D, Space, E, Esc | While skating: push, brake (in the air, lean forward or back), steer, crouch (let go to ollie), grind (in the air), stop |
| W then S | While skating: manual (then W / S balance it; A / D balance grinds) |
| F + W / S / A / D | On the ground or in a manual: manuals 2 to 5 (each character has five) |
| W / S / A / D with E | Onto a rail: grinds 2 to 5 (none: grind 1) |
| Q / F (+ W, S, A, D) | While skating, in the air: flip trick / grab (hold to keep grabbing) |
| Two directions, then Q (or E in a manual) | With the special meter full: the character's special grab (or manual) |
| E off a quarter pipe's lip | Lip trick on the coping (A / D balance, Space drops back in) |
| R as you land from vert | Revert (keeps the combo going into a manual) |
| Tab | While skating: to the next spawn point |
| J / L | While skating: look round (springs back) |
| P | While skating: pause (Resume, Restart, Stop skating) |
| M | While skating: show or hide the map (the level from above round the skater, drawn from its collision, with collectibles, letters and warps marked) |
| C | While skating: the next of the game's chase cameras (`Skater_Camera_Array`: near, standard, far, standard LTG) |
| F12 | Save a picture of the window to `Pictures\DESA Map Viewer` |
| P | Play or pause the character |
| [ and ] | Previous or next animation |
| F1 | Hide or show the panel |

A gamepad works too while skating, laid out like the GameCube game: the
left stick or D-pad steers (up pushes, down brakes; in the air they lean), A crouches and ollies,
X flips, B grabs, Y grinds, the shoulder buttons revert, the right stick
looks round and Start pauses. It rumbles
where the game's does (ollies, landings, grinds, reverts, flails, bails);
the panel's Rumble box turns that off, and the `pad_check` example
checks that a pad can rumble.

The panel's **Simplified controls** box (or the game's Options > Controls >
Pro Controls, off) plays the game's simplified scheme: the trick tables'
`Simplified` entries, where any trick button does the direction's trick,
grinds and lips too, and landing mid-trick loses the combo rather than
bailing. Characters stand at their appearances' sizes (Jessie 1.12, the
kids 0.85). The created **Kid**, boy or girl, is dressed from the Skate
Shop's part lists, under the panel's Kid's clothes or through the game's
own Edit Skater, Create-A-Skaters (six saved kids) and Extreme Skate
Crew menus in the shop, with the parts' colours, the board's deck and
griptape, and the kid's body shape. Zurg's Home's low gravity zones work
as its scripts set them up (a goal turning on `CHEAT_MOON`).

The panel's **2 minute run** is the game's single session (Trick Attack):
two minutes from the level's start, the last combo counting once it
lands, and the best score for each level and character kept in the
settings file; Replay plays the run back as it was shown. **S-K-A-T-E
letters** plays the level's letters goal: its settings come from running
the level's `<level>_AddGoal_SKATE` script and catching what it hands
`GoalManager_AddGoal` (the time, the letters' objects, the restart node),
the letters spin and bob with the goal's own scripts, and each is picked
up within 8 feet, as `SkateLetter_InitLetter` sets. `--letters` with
`--skate` screenshots it. The **High Score** and **Pro Score goals** read
`AddGoal_HighScore` and `AddGoal_ProScore` the same way: the score (a
global such as `pride_highscore_score`), the time and the goal's name;
`--score-goal high|pro` screenshots one. **Collectibles** are the
character's `AddGoal_Collect25` objects (`jessie_collect25_objects`: 25 on
each level of its world), shown spinning (`Obj_RotY speed = 250`) and
hovering (`Obj_Hover`) while skating, picked up within 7 feet with the
gap sound, and kept collected in the settings file. **Warps** come from
the levels' scripts too: the Hub's portals are the sectors whose scripts
call `LevelWarp level_num = N filmStrip = ...` (the level being the `N`th
of `level_select_menu_level_info`), opening as the skater comes within 60
feet; each level's `Warp_Master_Hub` (`HubWarp`, the Kid's way back in the
game) is shown to everyone with the game's own sparkle
(`TRG_Warp_Particle_Hub`), and each Hub portal starts its `warpParticle`
as it opens. **Particle effects**
(campfires, steam) come from the levels' `ParticleEmitter` nodes: their
scripts' `CreateParticleSystem` names an emit script, which the
interpreter runs (`setlife`, `setspeedrange`, `setemittarget`,
`setcolor`, `emit`...) to throw soft glowing blobs; most emitters are
made later by goals, so the ones there at the start show. `--run SECONDS`
with `--skate` screenshots one with that much time left (with `--goal`,
the goal's clock), and `--replay-at SECONDS` the replay of it that far in.

**Goals** start as the game's do. What the goal asks (its `goal_text`)
bounces up mid-screen once its start scripts have run and its pro has had
their say, then shrinks to the top right and stays
(`create_panel_block ... style = panel_message_goal`). Goals flagged
`use_hud_counter` get the game's counter box ("0 | 12 Weak Spots",
`goal_create_counter`), which pops as the count changes. Won, the goal's
name, a check and "Complete!" (`goal_success_messages`); lost, an X and
"Incomplete!" (`goal_fail`). `--goal KIND` with `--skate` screenshots
one (`--goal Gaps`, `--goal Collect2`...). Their pros offer them with the game's speech box
("Army Man: Press B to talk."): B on the pad or Enter starts the goal,
and you skate on while it's offered.

**Two players** are switched off on the main menu for now: its 2 Player
is greyed out (`not_focusable`, as the game's demo builds have it) while
the games are being finished. `--two-player` still plays them for
screenshots. As built, they come in through the game's own menus: the
main menu's 2 Player (`start_2p`, `SetGameType freeskate2p`), its level select, then
the skater select twice ("PLAYER ONE", Ready; "PLAYER TWO", Ready). The
level arrives split in two, one player over the other, under the game's
TWO PLAYERS menu (Start Game, Change Level, Split Mode for side by side,
Exit to Stage), and paused the game's 2-player pause menu has the same.
Each player skates with their own physics, tricks, score and chase
camera, and sees the other. Player two plays on the second gamepad, or
the arrows and the numpad (0 ollie, 1 flip, 2 grab, 3 grind, 5 revert;
or Right Shift , . / ;). `--two-player CHARACTER` with `--skate`
screenshots it (`--side-by-side` for the other split).

Start Game opens the game's GAME OPTIONS, with its three game types
(`mp_game_type_info`) and each one's settings, kept as the game keeps
its `splitscreen` preferences:

- **Horse** (time limit 10/20/30 seconds, and the word: PIG, COW, GOAT,
  HORSE, DONKEY or GIANT SQUID). One player at a time on the whole
  screen, at the level's Horse spots. The first sets a score, and the
  other has to beat it. The clock runs until a trick starts. Lower
  scores get letters, and spelling the word loses ("Woody is a PIG!").
  The game's own messages, score panel and sounds are used.
- **King of the Hill** (time limit). The game's star (`crown.mdl`) waits
  at one of the level's crown spots. Whoever reaches it carries it over
  their head ("You are king of the hill!") until they bail and drop it.
  Each player's time with it counts, and an arrow shows the way to it.
- **Score Challenge** (target score). Combos add up, and the first to
  the target wins ("Score Challenge: 50,000 pts to win").

King and Score Challenge end on the game's RANKINGS, and its Done goes
back to TWO PLAYERS. The pause menu's End Current Game does too.
`--versus horse|king|scorechallenge` with `--two-player` starts one for
a screenshot. Not yet: in King, knocking the king over by skating into
them. Each half shows the viewer's own score and combo rather than the
game's panel.

`desa-map-viewer --level HUB --screenshot out.png` renders one frame,
panel included, without opening a window. Add
`--character jessie --animation Ollie --time 0.4` to show a character
there, with the camera looking at them, or `--camera-path Cam_Plane
--time 8` to view from a camera path. For screenshots to share, `--clean`
leaves out the panel and markers, `--size 1920x1080` sets the size,
`--spawn N` picks the spawn point, `--orbit`, `--distance` and
`--camera-height` place the camera around the character, and `--lift`
raises them into the air (jump height comes from the game's physics, not
the animations). `--camera x,y,z,yaw,pitch` and `--object NAME` place the
camera directly, and `--look` sets the Advanced graphics as NAME=VALUE
pairs (`--look sky=1,clouds=0.45,hour=17.8,fog=200,rain=0.6,water=0.6`).
The pictures in `docs/screenshots` were made that way.

## Command-line tools

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

## Crates

| Crate | What it does |
|---|---|
| `gc_disc` | Reads GameCube disc images: header, file system table, `main.dol` sections |
| `prg` | Reads the `.prg` archives (Neversoft PRE format, LZSS-compressed) that hold almost all game data |
| `ngc_texture` | Decodes `.img.ngc` images and `.tex.ngc` texture dictionaries (GX CMPR and RGBA8) |
| `ngc_model` | Parses `.mdl.ngc` models, `.skin.ngc` skinned characters and `.scn.ngc` level scenes: materials, vertex arrays, bone weights, triangle strips |
| `ngc_anim` | Parses `.ske` skeletons and `.ska.ngc` animations, samples them and poses skinned characters |
| `ngc_collision` | Parses `.col.ngc` collision meshes and their BSP trees, repairing the fields the original tool corrupted |
| `qb` | Tokenizes, decompiles and parses Neversoft QB scripts (level node arrays, game logic) |
| `skate` | Skater physics: the game's constants and character stats, ray casts against collision, the skater |
| `desa_viewer` | Level, object and character rendering (wgpu), plus `desa-map-viewer` (the Map Viewer) and `desa-viewer` (command-line viewer and screenshots) |
| `desa_cli` | The `desa` command-line tool built on top of them |
