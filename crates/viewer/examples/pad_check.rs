//! Lists the connected gamepads and whether each can rumble, then rumbles
//! them for half a second.
//!
//! `cargo run --release -p desa_viewer --example pad_check`
use gilrs::ff::{BaseEffect, BaseEffectType, EffectBuilder, Repeat, Replay, Ticks};

fn main() {
    let mut gilrs = gilrs::Gilrs::new().expect("gamepad support");
    // Let the pads connect.
    for _ in 0..20 {
        while gilrs.next_event().is_some() {}
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let mut rumbles = Vec::new();
    for (id, pad) in gilrs.gamepads() {
        println!("{}: {} (rumble {})", id, pad.name(), pad.is_ff_supported());
        if pad.is_ff_supported() {
            rumbles.push(id);
        }
    }
    if rumbles.is_empty() {
        println!("no gamepad that can rumble");
        return;
    }
    let effect = EffectBuilder::new()
        .add_effect(BaseEffect {
            kind: BaseEffectType::Strong { magnitude: 40_000 },
            scheduling: Replay {
                play_for: Ticks::from_ms(500),
                ..Default::default()
            },
            ..Default::default()
        })
        .gamepads(&rumbles)
        .finish(&mut gilrs)
        .unwrap();
    effect.set_repeat(Repeat::For(Ticks::from_ms(500))).unwrap();
    effect.play().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(700));
    println!("rumbled");
}
