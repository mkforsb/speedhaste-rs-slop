# Speed Haste — Rust port: work log & TODO

A modern Rust port of *Speed Haste* (DOS, 1995, Javier Arévalo / Noriaworks).
Targets: native Linux (SDL3) and web (Dioxus + WebAudio), sharing one core crate.
The registered data blob `SPEEDH.JCL` is **never** shipped; the user provides it.
The shareware release's `SPEEDH.JCL` (freely copyable) is in `shareware/` and
built into the binaries as a fallback (`Jcl::shareware()`).

## Layout

```
crates/core     speedhaste-core   — everything platform independent: data formats,
                                    fixed-point math, game logic, software renderer,
                                    audio mixer + S3M player, GPU scene extraction.
                                    No platform deps.
crates/gpu      speedhaste-gpu    — wgpu renderer for race scenes + frame presentation
                                    (native Vulkan/GL and web WebGL2), texture replacements.
crates/native   speedhaste        — SDL3 frontend (video, audio, keyboard). `speedhaste [PATH/TO/SPEEDH.JCL]`
crates/relay    speedhaste-relay  — WebSocket relay server for network races.
crates/web      speedhaste-web    — Dioxus frontend (file picker, canvas, WebAudio, touch controls).
shareware/      the shareware SPEEDH.JCL, embedded by the core crate.
re/             reverse-engineering scripts, notes, Ghidra/REA config (no game data).
```

## Key design decisions

- **Faithful first.** The core reproduces the original 8-bit software renderer
  (Mode-7 floor, sector walls, scaled RLE sprites, textured/gouraud polygons) into a
  palettized framebuffer, with the same integer/fixed-point maths (wrapping `i32`
  arithmetic, the original sin/cos/atan tables, the same RNG). Physics and AI are
  thus bit-compatible with the original where practical.
  Known quirks kept on purpose (checked in DOSBox): coasting barely slows
  the car — off the gas, `revo` only loses the gear's `accel / 8` per tick
  (no speed-dependent drag; 6th gear's accel is 1/16 of 1st's), so a lap of
  Racer's Edge coasts from 140 to ~110 km/h — and that decay is skipped
  while a Stock car slides (userctl.c), so hard turns can lose less speed
  than going straight.
- **Async game flow.** The original is written as nested blocking loops (menus wait
  for keys, fades spin on VBL). Browsers can't block, so the game flow is ported as
  Rust `async` code: each blocking wait becomes `next_frame().await`. The platform
  polls one future per display frame. This keeps the port structurally close to the
  C source (easy to compare) with no threads/SharedArrayBuffer needed on the web.
- **70 Hz timebase.** The original timer runs at 70 Hz (VGA mode 13h refresh) and
  the race simulates at 70 ticks/s independent of rendering. The core keeps that
  clock; platforms render at whatever refresh rate they have.
- **Upgrade path.** The renderer works on a `Screen` of arbitrary size and the
  original already scales everything by integer `GL_ScreenXRatio/YRatio` (it had a
  640x400 SVGA mode), so higher resolutions come almost for free. Rendering reads
  game state but never mutates it, so an alternative GPU renderer (wgpu/three.js)
  can later consume the same state for filtering/texture replacement. Input is
  DOS scancodes plus platform-neutral gamepad state (`pad.rs`).
- **Gamepads: modern, not faithful.** Pads aim for "plug in an Xbox/DualShock
  pad and play", not the original's calibrated 2-button joystick. Pad *n*
  drives player *n* by default (Setup → Controls can pin a player to
  "Gamepad 1/2", saved as the original's Joystick A/B). In menus the pad
  produces arrow/Enter/Esc key presses; in races it steers through the
  original's analog path (`analog_x`, with the stick's dead zone remapped
  above the game's own ±4) and uses digital controls for the rest, so it
  never presses a keyboard player's keys. Mapping in `pad.rs`'s doc comment.
  Rumble (port addition): crashes into walls/cars and off-road bumping
  queue requests in `World::rumble`; the race maps them to the player's pad
  (SDL3 `set_rumble`, web `vibrationActuator.playEffect`). The controls menu
  drops the original's no-op "Calibrate Joystick" entries.
- **Higher resolutions** (`-scale N`, N > 2): the 3D views render natively at
  320N x 200N (the renderer already scaled by integer ratios). The original's
  HUD layouts only exist for 320 and 640, so above 2x the overlays (HUD,
  countdown, messages) are drawn with the 320 layout on a downsampled copy
  of the frame and only the pixels they change are blown up N x N — crisp
  pixel art, and the translucent minimap stays right (just blocky). Sky and
  unscaled marker sprites are grown by N/2 to keep the 640x400 look. Scale
  1 and 2 are untouched (still pixel-exact).
- **GPU renderer** (`-gpu`, web `?gpu`): when the platform asks
  (`Game::set_gpu`), races and the attract demo hand over a `Scene`
  instead of pixels: `race/render/scene.rs` extracts camera-space
  triangles (walls, billboards, meshes) reusing the software renderer's
  object selection, mesh visibility, lights and face order; the HUD and
  messages are drawn twice on 320x200 (over 0 and over 255) to tell
  opaque, darkened (minimap) and see-through pixels. `crates/gpu` draws
  that with wgpu at the target's resolution:
  - textures stay palettized (R16Uint atlas, 2D array) and are shaded
    through a *lit palette* (palette through each GRAFS.CLR row, rebuilt
    every frame, so VGA fades just work); sky and the Mode-7 floor are
    per-pixel passes using the original's formulas (floor and sky match
    the software pixel for pixel at 320x200);
  - hidden surfaces: walls use true depth, objects their centre depth
    with faces nudged by the software's face order — i.e. the software's
    painter's order between objects (billboards and particles would
    otherwise sink into cars); mesh faces are near-clipped on the CPU at
    the software's plane (4000);
  - translucent faces multiply by the mean luminance ratio of their
    shade row; optional bilinear filtering with smooth shading/fog;
  - only textures + uniforms (no storage buffers), so WebGL2 works. GL
    turns 1-layer textures into plain 2D ones: atlases always get 2+
    layers.
  Menus and software-rendered races are presented by the same renderer,
  so `-gpu` can switch races between renderers at run time (Ctrl+G).
- **Texture replacements** (`-gpu -textures DIR`): PNGs named after the
  game data (IS2 name for sprites/walls, `I3DNAME_n` for mesh material
  n, `GRAFSnnn` floor tiles — turned per map cell — `NUBESn`/`MOUNTn`),
  any size, alpha = holes; `dump_textures` writes the originals with
  those names. True-colour texels can't go through the colour tables,
  so each table row is fitted as an affine colour map (scale + offset per
  channel, least squares over the palette, refit per frame): the
  originals fed back as replacements render within 1.7/255 on average.
- **Network races** (`-relay ws://HOST:8790/ROOM`, web `?relay=...`, then
  Multiplayer Race → Start/Join): `net.rs` ports game/net.c's game side —
  lockstep like the original: everyone simulates the whole race from the
  master's setup and seed, only the humans' per-tick controls travel, a
  tick is simulated once every human's controls for it are known
  (`Player::clock`), and the local player is sampled at most 10 ticks
  ahead of the slowest remote (RACE_HandleComms). The lobby keeps the
  original's steps (SYNC/START, GAMEINFO, NET_SyncStart before each
  race, BYE), up to 4 nodes. IPX and serial/modem are replaced by a tiny
  relay (rooms by URL path; each message goes to the room's others with
  the sender's id) because browsers can only do WebSockets; a reliable
  ordered transport also makes the original's resend logic unnecessary.
  The platform hands the core a `Pipe` of binary messages. Port
  additions: state hashes every 35 ticks ("Out of sync"), "Waiting for
  players", the master's decoration detail for all, and a race ends for
  everyone when a node leaves (the original would hang). Setup →
  Multiplayer (modem phone numbers in the original) shows the server.
- **The software renderer stays.** Even once a GPU (GL/wgpu) renderer exists,
  the faithful 8-bit software renderer remains a first-class, selectable path
  (reference output, low-end fallback, pixel comparisons against the original).
  GPU work goes beside it, never replaces it.
- **Web presentation** uses a plain 2D canvas for the faithful output; with
  `?gpu` the canvas goes to wgpu's WebGL2 backend instead, sized to its
  on-screen size in device pixels. Three.js was dropped in favour of the
  wgpu renderer shared with native (the .wasm grows from 0.8 to 3.6 MB,
  1.4 MB gzipped, mostly wgpu + naga).

## Reverse engineering & REA

The partial original source (https://github.com/TheJare/SpeedHasteSrc) turned out to be
nearly complete for the game itself (it's the later "Circuit Racer" rebrand of the
same code). Missing: the VTAL sound library core (S3M player/mixer), plus small
bits. REA (Reverse Engineer Anything, https://github.com/morluto/rea) is used to
check the shipped `SPEED.EXE` against the source and to recover what's missing.

### Getting REA to analyze a DOS/4GW binary

1. `SPEED.EXE` is an MZ stub + **LE** (linear executable) for DOS/4GW. REA's Ghidra
   adapter explicitly rejects LE/LX (it supports MZ, COM, raw, ELF, PE, ...).
   REA's raw-binary mode maps file offset 0 at the entry point, which doesn't fit a
   two-object LE image either.
2. Solution: `re/scripts/le2elf.py` parses the LE object/page tables, applies all
   internal fixups (9259 32-bit offset fixups + 1 selector) and writes a static
   i386 ELF with both objects at their preferred bases (code 0x10000, data 0x60000).
   Ghidra (through REA) loads it as a normal ELF: 739 functions, 670 strings.
3. Ghidra has no Watcom compiler spec. Watcom's register convention (args in
   EAX, EDX, EBX, ECX; callee saves everything) is added via
   `re/ghidra-watcom/x86watcom.cspec` + `install.sh`, selected with
   `REA_GHIDRA_COMPILER_SPEC_ID=watcom` (REA validated and accepted the custom id).
4. The REA CLI starts a fresh Ghidra import per command (~22 s each). To keep one
   analyzed session alive, `re/scripts/rea_daemon.py` runs `rea --mcp` (stdio MCP)
   and exposes it on a Unix socket; `re/scripts/reaq.py TOOL '{json}'` sends single
   tool calls (open_binary, search_strings, xrefs, procedure_pseudo_code, ...) and
   prints the `normalized_result`. After the first query, calls take ~30 ms.

```sh
. re/rea-env.sh
python3 -I re/scripts/le2elf.py refs/full-game-dos/SPEED.EXE re/out/speed.elf
python3 -I re/scripts/rea_daemon.py $PWD/re/out/rea.sock &
python3 -I re/scripts/reaq.py open_binary '{"path":"'$PWD'/re/out/speed.elf","provider_id":"ghidra"}'
python3 -I re/scripts/reaq.py search_strings '{"pattern":"create player"}'
python3 -I re/scripts/reaq.py procedure_pseudo_code '{"procedure":"0x3d2b8"}'
```

First result: string "Can't create player" → xref 0x3d446 → `FUN_0003d2b8`, which
decompiles to an exact match of `THN_AddThing()` in `things.c`.

### REA findings log

| Address | Identified as | Notes |
|---------|---------------|-------|
| 0x3d2b8 | `THN_AddThing` | matches source |
| 0x28bb8 | `INTRO_DoIntro` | **differs**: shipped intro is friend → noria → fire; the source's first "wwsplash" screen (not in the data) is absent. `DAT_0006827c` = `LLK_LastScan`. |
| 0x28a94 | `Intro1` | matches source incl. the `(320-w)/2 - 10` x offset (clamped to 0 by `BM_Draw` for full-screen images) |
| 0x1d868 | `MENU_Init` | **differs**: 10 logo sprites `sphl%c.IS2` (source: 12, `sphl%i`) |
| 0x68394 | `LogoPos` | **differs**: 10 positions for S-P-E-E-D / H-A-S-T-E |
| 0x117a7.. | `main` (fl2.c) | matches: `LLS_Init(1,0)` (0x1c530, BIOS mode 13h) then `VGA_GetPalette` (0x1cd70) of colours 0-15/240-255, `VGA_ZeroPalette` (0x1cd9a) |
| 0x1ce98 | `VGA_FullFadePalette` | not in the source; limit `0x41 - pos` → ends one short of the target |
| 0x1ce1d | `VGA_FullFadeOutPalette` | not in the source; limit `0x40 - pos` → reaches the target |
| 0x1cdb2 / 0x1cde5 | `VGA_FadeOutPalette` / `VGA_FadePalette` | ±1 step per call |
| 0x405e4 | `UCT_GetUserControl` | not in the source; key bits as the port, joystick A/B: x, -y, button edges → GEARUP/GEARDN |
| 0x24f90 | `UpdateJoystick` (race.c) | matches source; `GL_JoyAX` 0x6780c.. |
| 0x29444 | `INTRO_DoGreets` | matches source (starts with AddStar5, table at 0x686c8); strings and fontgrt.fnt are shipped |
| 0x41b0c.. / 0x4db00.. | off-road dust (players / robots) | Ghidra didn't make functions of 0x406d4–0x41d0f (reached via pointers); read with objdump. Dust offset: y's random drawn **before** x's (port had x first; fixed). Robot target jitter: tx then ty (matches) |
| 0x4a728 | VTAL S3M loader | reads only ord/ins/pat counts, speed, tempo; **ignores global volume, master volume + stereo bit, default pans**; channel pan = chset bit 3 ? +127 : -128; song flags = 5 (fast volume slides, tone porta on tick 0); effect table drops V, U, I, S2x, T<0x31; EEx/FEx → fine (x+2)>>2 |
| 0x48a9c / 0x49764 | VTAL row / tick processing | volume-slide memory per channel (D/K/L only); volume capped at 63; vibrato also on tick 0 |
| 0x4a0ec | VTAL poll | acc += tempo*50 per mix tick; divisor RealTickRate*125>>8 → standard ST3 tempo |
| strings | menus | all menu texts of the source (`MainMenu`, `RaceTypeMenu`, `ConfigMenu`...) exist in the binary → same menu tree |

Open REA questions: shipped `MainMenu` (not needed so far: the menus match
pixel for pixel).

REA friction worth noting: LE unsupported (wrote le2elf.py), no Watcom cspec
(added one), ~22 s CLI round trips (wrote the MCP socket daemon), decompiler
names register args `unaff_*` because of the callee-saves-all convention, and
string search results sometimes include stray leading bytes — and occasionally
miss a string entirely when Ghidra merged it into a preceding one ("Mr. Shy" is
in the binary but `search_strings` didn't find it; a raw byte dump did).

## Data formats (all inside SPEEDH.JCL)

- **JCL**: archive; trailer at EOF-16: magic `0xdf73b489`, nfiles, realnfiles,
  dirsize. Directory of 32-byte entries (name[24], offset-from-dir-start (backwards), size).
- **PAL**: 768 bytes, 6-bit VGA RGB. **PIX**: raw 320-wide 8-bit images.
- **CLR**: colormaps, 32 levels x 256 (light/fog translation tables).
- **GRAFS.DAT**: 252 floor tiles of 64x64.
- **MAPxx.DAT**: 2+2 byte header, 64x64 u16 tile indices, 64x64 u8 rotations, then
  (x,y,angle,type) u16 thing records. **.SEC**: text sector map. **.PTH**: AI path.
- **IS2**: RLE sprite (column- or row-major), 24-byte header.
- **I3D**: "B3D" binary dump of Watcom `O3DM_TObject` (pointers as offsets), then textures.
- **FNT**: fonts. **RAW**: 8-bit unsigned PCM. **S3M**: Scream Tracker 3 music.

## Work log

- **Session 1**: Explored refs. Set up git, REA + Ghidra 12.1.4 + JDK 21,
  LE→ELF converter, Watcom cspec, REA MCP daemon. Read most of the game source.
  Core crate: data formats, fixed-point maths, async runtime, SDL3 frontend.
  Ported the race end to end (renderer, physics, AI, HUD), S3M player (checked
  vs libopenmpt), the whole front end (intro, menus, records, credits, demo),
  and the Dioxus web frontend (tested in headless Chromium). DOSBox reference
  harness: main menus pixel-identical. REA used to find shipped-vs-source
  differences (intro sequence, logo table) and to verify polygon code.
- **Session 2**: Fixed texture garbling on cars (tail lights when turning,
  rims, decals; reported from play testing). Cause: the texture fillers pack
  (tx, ty) as `SHL EBP,8` + `MOV BP,WORD PTR [tx+1]`. The `MOV` *replaces*
  the low word, but the port OR-ed it in, so ty's low fraction bits leaked
  into tx's integer bits. With `pack()` masking the high word, the car-select
  small cars now match DOSBox **pixel for pixel** (was ~90 differing pixels),
  which also validates the polygon filler used in races.
  GamePal colours 0-15/240-255 (placeholders in grafs.pal) come from the DAC
  after LLS_Init's BIOS mode set, i.e. the **VGA BIOS default palette**, with
  or without `-nologo`; the port had zeroed them. Found by mapping the
  course-select diff to palette indices (15 → white, 246/247 → dark grey),
  then matching against DOSBox's built-in table. REA read the shipped VGA
  fade routines (missing from the source); the full fade towards RGB uses
  64 - pos, not 65 - pos. Course select now differs only in the record time.
  Gamepad support (SDL3 gamepads, web Gamepad API standard mapping). REA
  showed the shipped `UCT_GetUserControl` (0x405e4) and `UpdateJoystick`
  (0x24f90); per the user, the pad design favours modern ergonomics over
  the joystick model. Tested headless (`pad:` script step), with an SDL
  virtual gamepad, and in Chromium with a stubbed `getGamepads()`.
  Web: SPEEDH.JCL is remembered in IndexedDB (Play / Forget it on the
  loader). Audio goes through an AudioWorklet: the mixer stays on the main
  thread (no SharedArrayBuffer/COOP needed), the frame loop keeps ~80 ms
  queued, the worklet reports frames played; ScriptProcessorNode remains as
  the fallback where AudioWorklet is missing (non-secure contexts).
  Ported the hidden greetings starfield (hold I+G+N in the main menu); REA
  confirmed it ships. The original reads one past its 1/z table for stars
  at exactly ZMAX; the port extends the table instead.
  RNG order in the off-road dust spawn: read the shipped code with objdump
  (Ghidra hadn't split those functions out); y's offset is drawn first.
  Tip: `objdump -d -M intel re/out/speed.elf` + grep for the inlined RNG
  (`add eax,0xb35fa137`) finds every random draw quickly.
  VTAL music: read the S3M loader/player with REA and recorded the original
  in DOSBox (SB Pro, `re/scripts/dosbox_audio.sh`). The player now does what
  VTAL does: every song hard-panned by channel setting (MENUS, SCIRCUIT,
  HASTE5/8 were mono before), PanWidth applied as /256, master/global volume
  ignored (HASTE2 and FIN were half volume), fast volume slides, tone porta
  and vibrato on tick 0, volume ≤ 63. Against the capture our L/R balance
  now correlates (0.41, was 0) and the side/mid ratio matches. HASTE9: ours
  and libopenmpt match the original equally (~0.70, the 8-bit capture's
  ceiling), so its lower libopenmpt score wasn't a port bug.
- **Race is pixel-exact vs the original.** Ran the original under `faketime`
  (fixed BIOS clock → fixed RND seed), drove it into a single race, captured
  frames, then brute-forced our `-seed` (= the original's BIOS_Clock at
  init, here 1129) with the `race_seed` example. Two fixes got every frame
  to 0 differing pixels: the column drawer is now a register-faithful port
  of vertdrwa.asm walking the IS2 RLE data (its transparent-run skips round
  up and the fraction's low word doubles as the pixel counter, so it samples
  differently from plain stepping — visible in the speckled grandstand
  textures and scaled sprites); and the remaining "differences" were DOSBox
  screenshot tearing (rows 0-99 of tick t over rows 100-199 of tick t+2).
  So physics, AI, RNG use, cameras, HUD and the renderer all match over
  11 s of racing with 19 AI cars.
  Music/effects balance: recorded the original's race with music muted
  (MusicVolume 0) and compared effect levels with ours on the same seed:
  ours were 0.68 of the original vs 0.45 for music, so the music level went
  up 1.5×. Found on the way: the original ignores `MusicOn No` once the
  first song starts (MENU_InitMusic always applies MusicVolume); the port
  keeps honouring MusicOn (deliberate deviation; the game's menus can't
  change it anyway). Headless gained `audio:PATH` and `HEADLESS_CFG`.
- **Session 3**: GPU renderer, end to end. Core: races and the demo build
  `Scene`s when asked (overlays classified by drawing them twice), F12 is
  off under the GPU (it composes at 320x200). Found while checking the
  scene maths: the projection is `focus/16` for walls, meshes and floor
  alike (sprites used `focus`), now `SceneView::focus`. New `crates/gpu`
  (wgpu 30): first headless comparison already matched the software
  layout; floor/sky pixel-identical, remaining differences are texture
  sampling details (perspective-correct vs the original's affine fillers
  and RLE column stepping). Dust particles disappeared into the cars with
  a plain z-buffer → objects now sort by centre depth like the software's
  painter. Filtering showed seams from wrapping (sky top, wall tops,
  sprite edges) → per-axis clamp flags. Native: wgpu surface on the SDL
  window (wgpu 30 wants the display handle on the instance; SDL's Window
  isn't Send, so its raw handle is wrapped), tested under Xvfb +
  lavapipe with xdotool. Web: WebGL2 backend, tested in headless
  Chromium; textures were black at first (1-layer array → GL 2D
  texture). Texture replacements with an affine fit of the colour table
  rows for lighting/fog, round-trip checked with the dumped originals,
  and with 4x-upscaled ones. No REA work this session: the GPU path
  builds on the already-verified software renderer.
  Network play (the last TODO item, first marked "not planned"): the
  released game/net.c and race.c's RACE_HandleComms were complete enough
  to port the protocol (no REA needed). The first two-node test desynced
  as soon as the cars moved: remote players had no key bindings, and gear
  changes compare the tick's scancode with the player's keys — which is
  why the original's SYNC packet carries everyone's keys. Then a stale
  hash from the end of race 1 failed a check in race 2 of a championship
  → messages carry the race number. Verified: 3 in-process nodes with
  100 ms latency (306 checks, 0 failed), championship continuation,
  leaving mid-race, two native windows through the real relay, and a
  native host racing a Chromium client in lockstep (x86-64 vs wasm32
  identical).
- **Session 4**: Shareware and touch. The shareware SPEEDH.JCL is the
  registered one minus circuits 3-8 (map02-07 .dat/.pth/.sec), with its
  own SPHLOGO.PIX and an ENDTEXT.BIN (the 80x25 text screen fl2.c copies
  to video memory on exit). The menus had already
  ported every GL_Registered check but the flag was hardcoded true; now it
  is `Data::is_registered()` (map07.sec present, as fl2.c). Headless runs
  confirmed the original's shareware behaviour: Championship shows
  "REGISTERED VERSION ONLY", circuits 3+ and cars 2+ blink "Registered
  Version" and refuse Enter, the demo races circuit 1. The JCL is
  `include_bytes!`'d in core and borrowed (`Jcl` holds `Cow<'static,
  [u8]>`), so no 6 MB copy at startup; the web wasm grows to 9.7 MB
  (4 MB gzipped). Native falls back to it with no path and no
  ./SPEEDH.JCL; the web start page has "Play Shareware Version".
  Touch controls (web only), after the stunt-car project's overlay: a
  virtual gamepad merged into pad 1 (`touch.rs`), so races and menus
  needed no changes beyond `Game::in_race()` to switch layouts. Sized
  for the black bars beside the 4:3 picture on a phone held sideways;
  upright, the picture moves to the top. Name entry gets the phone's
  keyboard through a hidden text field (kept holding one space so
  Backspace on an empty field still fires an input event). Tested in
  headless Chromium with CDP multi-touch (steer + gas held together) in
  both orientations; not yet on a real phone.
  Phone vibration, dropped again: the touch controls first passed the
  race's rumble requests (wall and car hits, strong motor only) to
  `navigator.vibrate(ms)`. Firefox for Android crashed on every wall hit
  with it — the whole browser, which a page shouldn't be able to do — and
  was fine with it switched off (user-tested). Not a call-rate problem:
  headless Chromium counted ~7 calls in 6 s of wall crashes. Could come
  back later, e.g. off by default or not on Firefox for Android; the
  rumble requests are still there (`Game::take_rumble(0)`) for gamepads.
  GAS and BRAKE became one pedal (gas in the top 65%, brake below,
  following the finger) after user feedback: the game barely slows off
  the gas (a kept quirk, see design decisions), so on a phone the brake
  must be one thumb-rock away rather than a lift and re-press.
  Exit text screen (`textscr.rs`): the shareware's ENDTEXT.BIN holds
  leftovers of an older text drawn green on green (attribute 0x22), so
  it has to be shown with its attributes; hidden cells become spaces.
  Without the file (registered data) fl2.c's DrawBar fills the top line
  with "SPEED HASTE v1.00" in 0x2E — that string is in the shipped
  SPEED.EXE next to "endtext.bin", but the bar itself wasn't compared
  with DOSBox. Native prints it on exit in 24-bit ANSI colour (plain
  text when stdout isn't a terminal); checked in xterm. The web page
  shows it as coloured monospace text in place of the canvas, with
  "Play again". Note the shareware's Quit first shows its graphical nag
  screen (the shareware SPHLOGO.PIX), which waits for a key.

## Building & running

```sh
# Native (needs libsdl3-dev): config/records go to $XDG_DATA_HOME/speedhaste
cargo run --release -p speedhaste -- /path/to/SPEEDH.JCL [-nologo] [-track N -car N]
# Without a path: ./SPEEDH.JCL, else the built-in shareware version.

# Web (needs `cargo install dioxus-cli`): serve the output dir, open, pick SPEEDH.JCL
dx build --package speedhaste-web --platform web --release   # run from the workspace root
python3 -m http.server -d target/dx/speedhaste-web/release/web/public 8765
# URL query = original options, e.g. http://localhost:8765/?nologo&track=3
# Touch controls: on where the primary pointer is a finger; ?touch=1 / ?touch=0
# or the start page's toggle (saved in localStorage) override.
# Higher race resolution (port addition): -scale N (web: ?scale=N) makes the
# SVGA setting / F12 use 320N x 200N instead of 640x400 (N = 2..8).
# GPU renderer (port addition): -gpu (web: ?gpu) draws races with wgpu at the
# window's resolution; Ctrl+G switches races GPU <-> software, Ctrl+F toggles
# filtering (-filter / ?filter to start with it). Native -textures DIR loads
# replacement PNGs (write the originals with the dump_textures example).
cargo run --release -p speedhaste -- SPEEDH.JCL -gpu -filter -textures DIR
# Network races: run a relay somewhere reachable, then every player starts
# the game with the same room URL and picks Multiplayer Race (one Starts,
# the others Join; -master / -slave skip that menu). Web: ?relay=URL (a page
# served over https needs a wss:// relay, e.g. behind a TLS proxy).
cargo run --release -p speedhaste-relay -- 0.0.0.0:8790
cargo run --release -p speedhaste -- SPEEDH.JCL -relay ws://HOST:8790/myroom
```

## Testing helpers

- `cargo run -p speedhaste-core --example headless -- SPEEDH.JCL SCRIPT OUTDIR [game args]`
  runs the game without a window at simulated 60 fps with scripted keys
  (`wait:N`, `key:NAME`, `down:NAME`, `up:NAME`, `shot:NAME`, `raw:NAME`,
  `pad:lx=-1/rt=1/SOUTH`), saving PPMs.
  Game args like `-track 3 -car 2` jump straight into a race.
- `cargo run -p speedhaste-core --example dump -- SPEEDH.JCL OUTDIR [meshes]` dumps assets.
- `re/scripts/web_test.mjs` drives the web build in headless Chromium (Playwright):
  loads SPEEDH.JCL through the file input, scripted keys and a fake gamepad (`pad:SPEC`),
  canvas screenshots.
- `re/scripts/web_touch_test.mjs` drives the touch controls on a phone-sized
  touch viewport (`VIEWPORT=390x844` for upright) with the built-in
  shareware: `tap:SELECTOR`, `hold:`/`release:SELECTOR`, `steer:DX` /
  `steer:off`, `click:TEXT`, `type:TEXT`, `shot:NAME`. Playwright's own
  browser isn't installed here; scripts use `/usr/bin/chromium`
  (`executablePath`, env `CHROMIUM`). Race from the menu:
  `tap:.t-enter,wait:1500,tap:.t-down,wait:300,tap:.t-enter,wait:3500,tap:.t-enter,wait:4000,tap:.t-enter`.
- `re/scripts/web_audio_check.mjs` checks the web audio output (AudioWorklet
  chunk pacing and loudness).
- `re/scripts/dosbox_audio.sh OUT.raw SECONDS -nologo` records the original's
  audio (SB Pro through VTAL's mixer); `re/scripts/cmp_capture.py` compares it
  with `cargo run --release -p speedhaste-core --example s3m_render -- SPEEDH.JCL
  SONG.S3M 22050 SECONDS PANWIDTH OUT.raw` renders.
- Pixel-exact race check: `faketime -f '@2026-01-01 00:01:00' dosbox -conf
  re/dos/dosbox_nl.conf` (the usual conf with `speed.exe -nologo`), then
  `dosbox.sh key Return; key Down Return; key Return; key Return` and `shot`s
  during the race; `race_seed SPEEDH.JCL SHOT.ppm FROM TO MAXTICKS` (release
  example) finds the seed and tick that match (BIOS_Clock ≈ 18.2 × seconds
  since midnight + boot ticks). Env `RACE_SEED_DUMP=1` saves the best frame,
  `RACE_SEED_SAVE_TICK=N` a given tick. Beware screenshot tearing.
- `re/scripts/cmp_audio.py` compares our S3M renders with libopenmpt's.
- `cargo run --release -p speedhaste-gpu --example gpu_shot -- SPEEDH.JCL OUTDIR
  FRAMES WxH [game args]` runs the game twice headless (software and GPU
  races, same args) and saves `soft_N.ppm` / `gpu_N.ppm` (GPU at WxH, on
  lavapipe here). Env `KEYS=N:NAME,...` taps keys at frames (into a single
  race: `KEYS=31:ESC,333:ENTER,365:DARROW,367:ENTER,619:ENTER,921:ENTER`,
  shots from ~1300), `GPU_FILTER=1`, `TEXTURES=DIR`. Audio is mixed each
  frame because menus wait for music fades.
- `cargo run --release -p speedhaste-gpu --example dump_textures -- SPEEDH.JCL
  OUTDIR` writes every replaceable texture as a PNG.
- `cargo run --release -p speedhaste-core --example net_test -- SPEEDH.JCL OUTDIR
  FRAMES SCRIPT_A SCRIPT_B [...]` plays a network race between several games
  in one process (in-memory relay, env `NET_DELAY=frames` latency) and prints
  the desync checks. Scripts: `N:KEY` tap, `N:+KEY`/`N:-KEY` hold/release,
  `N:shot:NAME`. Master into a single race / slave joining:
  `31:ESC,333:DARROW,335:ENTER,365:ENTER,395:DARROW,397:ENTER,619:ENTER,760:ENTER,1000:ENTER`
  and `31:ESC,333:DARROW,335:ENTER,365:DARROW,367:ENTER,1000:ENTER`.
- Native network runs print the checks with `SPEEDHASTE_NET_DEBUG=1`. Two
  native windows under Xvfb + a Chromium page can share one relay (see the
  session 3 log); in Playwright scripts leave ~300 ms between keys, or two
  taps land in the same frame.
- `re/scripts/dosbox.sh start|key|down|up|shot NAME|stop` drives the *original*
  game in DOSBox under Xvfb (`Xvfb :99`) and grabs 320x200 reference frames
  into `re/dos/capture/` (the game is copied to the gitignored `re/dos/game`).

## TODO

- [x] Workspace + core crate: JCL, palettes, PIX, IS2, I3D, fonts, maps
- [x] Fixed-point math, sin/cos/atan tables, RNG
- [x] Native SDL3 frontend (window, palette blit, keyboard, audio)
- [x] Web frontend (Dioxus file picker, canvas, WebAudio, localStorage)
- [x] Race renderer, simulation, AI, HUD, split screen, demo
- [x] Audio: mixer, effects, S3M player
- [x] Menus, intro, credits, records, config persistence
- [x] Pixel-exact car-select 3D cars → validates poly filler (session 2)
- [x] Pixel-exact race frames vs the original (session 2)
- [x] Gamepad input (native + web)
- [x] AudioWorklet, IndexedDB caching of the JCL
- [x] REA: VTAL music details, RNG argument order, HASTE9 accuracy
- [x] Greetings effect (hold I+G+N in the main menu)
- [x] Network multiplayer (WebSocket relay; native + web)
- [x] Higher race resolutions (`-scale N`)
- [x] GPU renderer alongside the software one (native + web), filtering
- [x] Texture replacements (native `-textures DIR`)
- [x] Built-in shareware data as a fallback (native + web)
- [x] Touch controls (web)
