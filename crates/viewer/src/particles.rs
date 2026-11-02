//! The levels' particle effects: steam, sparks, dust, glows.
//!
//! A `ParticleEmitter` node's `TriggerScript` makes a system with
//! `CreateParticleSystem ... emitscript = script max = n`; the emit script
//! then sets the particles up and emits them, on and off, with these
//! commands (run here with the QB interpreter):
//!
//! - `setpos pos = (x, y, z)`: where they come from;
//! - `setlife min max`: seconds each lasts;
//! - `setemittarget target = (x, y, z)`, `setanglespread spread` (radians
//!   off it) and `setspeedrange min max` (units a frame): which way they
//!   go, and how fast;
//! - `setemitrange width height` and `SetCircularEmit circular`: the patch
//!   they start in, square or round;
//! - `setforce force = (x, y, z)`: pull on them (units a frame, a frame);
//! - `setparticlesize sw sh ew eh`: size at the start and the end;
//! - `setcolor corner sr sg sb sa mr mg mb ma er eg eb ea midtime`: colour
//!   at the start, the middle (at `midtime` of the life) and the end, in
//!   the PlayStation 2 way where 128 is full;
//! - `emit num = n`.
//!
//! Units and scales are this module's reading of the values (speeds in
//! units a frame, at 60 frames a second), not checked against the game.
//! They're drawn as quads facing the camera with the level's particle
//! textures (`images/particles`), adding light or blending over as the
//! system's `blendmode` says.

use glam::Vec3;
use qb::vm::{Host, Outcome, Program, Thread};
use qb::{Value, checksum};

use crate::nodes::LevelNodes;

/// Frames a second (speeds and forces are per frame).
const FPS: f32 = 60.0;
/// Particles in one system at once, at most, whatever it asks for.
const MOST: usize = 300;

#[derive(Clone)]
struct Settings {
    pos: Vec3,
    life: (f32, f32),
    target: Vec3,
    spread: f32,
    speed: (f32, f32),
    range: (f32, f32),
    circular: bool,
    force: Vec3,
    size: (f32, f32),
    /// Start, middle and end colour (0 to 1), and when the middle is.
    color: [[f32; 4]; 3],
    midtime: f32,
}

struct Particle {
    position: Vec3,
    velocity: Vec3,
    age: f32,
    life: f32,
}

struct System {
    /// The emitter's node name.
    name: u32,
    /// Its texture's name (checksum) and whether it adds light.
    texture: u32,
    additive: bool,
    emitter: Vec3,
    thread: Thread,
    settings: Settings,
    particles: Vec<Particle>,
    max: usize,
    /// How far off it's drawn and run (units).
    range: f32,
}

/// The level's particle systems.
pub struct Particles {
    systems: Vec<System>,
    seed: u32,
}

/// Catches `CreateParticleSystem`: the emit script and the most particles.
#[derive(Default)]
struct Create {
    /// The emit script, most particles, texture and whether they add.
    made: Option<(u32, usize, u32, bool)>,
}

impl Host for Create {
    fn command(&mut self, _target: Option<u32>, name: u32, args: &Value) -> Outcome {
        if name == checksum("CreateParticleSystem") {
            if let Some(script) = args.get(checksum("emitscript")).and_then(Value::as_name) {
                let max = args
                    .get(checksum("max"))
                    .and_then(Value::as_int)
                    .unwrap_or(100)
                    .max(1) as usize;
                let texture = args
                    .get(checksum("texture"))
                    .and_then(Value::as_name)
                    .unwrap_or(0);
                let additive = args
                    .get(checksum("blendmode"))
                    .and_then(Value::as_name)
                    .is_none_or(|b| b == checksum("Add") || b == checksum("FixAdd"));
                self.made = Some((script, max.min(MOST), texture, additive));
            }
        }
        Outcome::Done(false)
    }
}

/// Runs an emit script's commands on its system.
struct Emit<'a> {
    system: &'a mut System,
    seed: &'a mut u32,
    /// Close enough to the camera to be worth making particles.
    near: bool,
}

fn random(seed: &mut u32) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    (*seed >> 8) as f32 / (1u32 << 24) as f32
}

fn number(args: &Value, key: &str) -> Option<f32> {
    args.get(checksum(key)).and_then(Value::as_f32)
}

fn vector(args: &Value, key: &str) -> Option<Vec3> {
    args.get(checksum(key))
        .and_then(Value::as_vector)
        .map(Vec3::from)
}

impl Host for Emit<'_> {
    fn command(&mut self, _target: Option<u32>, name: u32, args: &Value) -> Outcome {
        let c = checksum;
        let s = &mut self.system.settings;
        if name == c("setpos") {
            if let Some(p) = vector(args, "pos") {
                s.pos = p;
            }
        } else if name == c("setlife") {
            s.life = (
                number(args, "min").unwrap_or(s.life.0),
                number(args, "max").unwrap_or(s.life.1),
            );
        } else if name == c("setanglespread") {
            s.spread = number(args, "spread").unwrap_or(s.spread);
        } else if name == c("setspeedrange") {
            s.speed = (
                number(args, "min").unwrap_or(s.speed.0),
                number(args, "max").unwrap_or(s.speed.1),
            );
        } else if name == c("setemitrange") {
            s.range = (
                number(args, "width").unwrap_or(s.range.0),
                number(args, "height").unwrap_or(s.range.1),
            );
        } else if name == c("setforce") {
            s.force = vector(args, "force").unwrap_or(s.force);
        } else if name == c("setemittarget") {
            s.target = vector(args, "target").unwrap_or(s.target);
        } else if name == c("setparticlesize") {
            s.size = (
                number(args, "sw").unwrap_or(s.size.0),
                number(args, "ew").unwrap_or(s.size.1),
            );
        } else if name == c("setcolor") {
            // Corner 0 stands for all four.
            if number(args, "corner").unwrap_or(0.0) == 0.0 {
                let get = |k: &str| number(args, k).unwrap_or(128.0) / 128.0;
                s.color = [
                    [get("sr"), get("sg"), get("sb"), get("sa")],
                    [get("mr"), get("mg"), get("mb"), get("ma")],
                    [get("er"), get("eg"), get("eb"), get("ea")],
                ];
                s.midtime = number(args, "midtime").unwrap_or(0.5).clamp(0.01, 0.99);
            }
        } else if name == c("SetCircularEmit") {
            s.circular = number(args, "circular").unwrap_or(0.0) != 0.0;
        } else if name == c("emit") {
            let count = number(args, "num").unwrap_or(1.0).max(0.0) as usize;
            if self.near {
                for _ in 0..count {
                    if self.system.particles.len() >= self.system.max {
                        break;
                    }
                    let particle = spawn(&self.system.settings, self.seed);
                    self.system.particles.push(particle);
                }
            }
        }
        Outcome::Done(true)
    }
}

/// A new particle as the settings say.
fn spawn(s: &Settings, seed: &mut u32) -> Particle {
    let target = s.target.normalize_or(Vec3::Y);
    // Off the target by up to the spread, any way round.
    let side = target.any_orthonormal_vector();
    let up = target.cross(side);
    let angle = s.spread * random(seed).sqrt();
    let round = random(seed) * std::f32::consts::TAU;
    let dir = (target * angle.cos() + (side * round.cos() + up * round.sin()) * angle.sin())
        .normalize_or(target);
    let speed = s.speed.0 + (s.speed.1 - s.speed.0) * random(seed);
    // Somewhere in the patch (across the target).
    let (a, b) = (random(seed) - 0.5, random(seed) - 0.5);
    let (a, b) = if s.circular {
        let r = random(seed).sqrt() * 0.5;
        let t = random(seed) * std::f32::consts::TAU;
        (r * t.cos(), r * t.sin())
    } else {
        (a, b)
    };
    Particle {
        position: s.pos + side * a * s.range.0 + up * b * s.range.1,
        velocity: dir * speed * FPS,
        age: 0.0,
        life: (s.life.0 + (s.life.1 - s.life.0) * random(seed)).max(0.05),
    }
}

impl Particles {
    /// The level's particle systems there at the start: each emitter's
    /// script run to make its system.
    pub fn new(program: &Program, nodes: &LevelNodes) -> Self {
        let mut particles = Particles {
            systems: Vec::new(),
            seed: 0x1234_5679,
        };
        for emitter in nodes.emitters.iter().filter(|e| e.created_at_start) {
            particles.add(program, emitter);
        }
        particles
    }

    /// Starts the emitter with this node name (as `create Name = ...` does
    /// in the scripts), unless it's going already.
    pub fn start(&mut self, program: &Program, nodes: &LevelNodes, name: u32) {
        if self.systems.iter().any(|s| s.name == name) {
            return;
        }
        if let Some(emitter) = nodes.emitters.iter().find(|e| e.name == name) {
            self.add(program, emitter);
        }
    }

    /// Stops the emitter with this node name (`kill Name = ...`).
    pub fn stop(&mut self, name: u32) {
        self.systems.retain(|s| s.name != name);
    }

    /// Whether the level has an emitter by this node name.
    pub fn has_emitter(nodes: &LevelNodes, name: u32) -> bool {
        nodes.emitters.iter().any(|e| e.name == name)
    }

    fn add(&mut self, program: &Program, emitter: &crate::nodes::Emitter) {
        {
            let mut host = Create::default();
            let mut thread = Thread::new(emitter.script, Vec::new());
            thread.run(program, &mut host, 0.0);
            let Some((script, max, texture, additive)) = host.made else {
                return;
            };
            if !program.has_script(script) {
                return;
            }
            self.systems.push(System {
                name: emitter.name,
                texture,
                additive,
                emitter: emitter.position,
                thread: Thread::new(script, Vec::new()),
                settings: Settings {
                    pos: emitter.position,
                    life: (1.0, 1.0),
                    target: Vec3::Y,
                    spread: 0.2,
                    speed: (1.0, 2.0),
                    range: (0.0, 0.0),
                    circular: false,
                    force: Vec3::ZERO,
                    size: (10.0, 10.0),
                    color: [
                        [1.0, 1.0, 1.0, 0.5],
                        [1.0, 1.0, 1.0, 0.3],
                        [1.0, 1.0, 1.0, 0.0],
                    ],
                    midtime: 0.5,
                },
                particles: Vec::new(),
                max,
                // Seen from a good way off (the node's `CutOff` reads
                // short for inches).
                range: (emitter.cutoff * 6.0).max(3000.0),
            });
        }
    }

    pub fn len(&self) -> usize {
        self.systems.len()
    }

    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }

    /// Runs the emit scripts and moves the particles on, `dt` seconds;
    /// systems far from `eye` make none.
    pub fn update(&mut self, program: &Program, dt: f32, eye: Vec3) {
        for system in &mut self.systems {
            let near = system.settings.pos.distance(eye) < system.range
                || system.emitter.distance(eye) < system.range;
            if !system.thread.is_finished() {
                let mut host = Emit {
                    system: &mut *system,
                    seed: &mut self.seed,
                    near,
                };
                // (The thread is moved out to run it against its own system.)
                let mut thread =
                    std::mem::replace(&mut host.system.thread, Thread::new(0, Vec::new()));
                thread.run(program, &mut host, dt);
                system.thread = thread;
            }
            let force = system.settings.force * FPS * FPS;
            for p in &mut system.particles {
                p.age += dt;
                p.velocity += force * dt;
                p.position += p.velocity * dt;
            }
            system.particles.retain(|p| p.age < p.life);
        }
    }

    /// The particles as textured quads facing `eye`, by texture (checksum)
    /// and blending.
    pub fn quads(&self, eye: Vec3) -> Vec<(u32, bool, Vec<crate::renderer::ParticleVertex>)> {
        let mut out: Vec<(u32, bool, Vec<crate::renderer::ParticleVertex>)> = Vec::new();
        for system in &self.systems {
            if system.particles.is_empty() {
                continue;
            }
            let at = match out
                .iter()
                .position(|(t, a, _)| *t == system.texture && *a == system.additive)
            {
                Some(i) => i,
                None => {
                    out.push((system.texture, system.additive, Vec::new()));
                    out.len() - 1
                }
            };
            let s = &system.settings;
            let vertices = &mut out[at].2;
            for p in &system.particles {
                let t = p.age / p.life;
                let (from, to, k) = if t < s.midtime {
                    (s.color[0], s.color[1], t / s.midtime)
                } else {
                    (s.color[1], s.color[2], (t - s.midtime) / (1.0 - s.midtime))
                };
                let mix =
                    |i: usize| ((from[i] + (to[i] - from[i]) * k).clamp(0.0, 1.0) * 255.0) as u8;
                let color = [mix(0), mix(1), mix(2), mix(3)];
                if color[3] == 0 {
                    continue;
                }
                let size = (s.size.0 + (s.size.1 - s.size.0) * t) * 0.5;
                // Upright to the screen: right across the view, up from it.
                let to_eye = (eye - p.position).normalize_or(Vec3::Z);
                let right = Vec3::Y.cross(to_eye).normalize_or(Vec3::X) * size;
                let up = to_eye.cross(right.normalize_or(Vec3::X)) * size;
                let corner = |x: f32, y: f32, u: f32, v: f32| crate::renderer::ParticleVertex {
                    position: (p.position + right * x + up * y).to_array(),
                    uv: [u, v],
                    color,
                };
                let (a, b, c, d) = (
                    corner(-1.0, 1.0, 0.0, 0.0),
                    corner(1.0, 1.0, 1.0, 0.0),
                    corner(1.0, -1.0, 1.0, 1.0),
                    corner(-1.0, -1.0, 0.0, 1.0),
                );
                vertices.extend([a, b, c, a, c, d]);
            }
        }
        out
    }
}
