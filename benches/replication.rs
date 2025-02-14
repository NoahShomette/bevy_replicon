#[path = "../tests/common/mod.rs"]
mod common;

use std::time::{Duration, Instant};

use bevy::{app::MainScheduleOrder, ecs::schedule::ExecutorKind, prelude::*};
use bevy_replicon::prelude::*;
use criterion::{criterion_group, criterion_main, Criterion};

#[derive(Component, Reflect, Default, Clone, Copy)]
#[reflect(Component)]
struct DummyComponent;

const ENTITIES: u32 = 900;

fn replication(c: &mut Criterion) {
    c.bench_function("entities send", |b| {
        b.iter_custom(|iter| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iter {
                let mut server_app = App::new();
                let mut client_app = App::new();
                for app in [&mut server_app, &mut client_app] {
                    setup_app(app);
                }
                common::connect(&mut server_app, &mut client_app);

                server_app
                    .world
                    .spawn_batch([(Replication, DummyComponent); ENTITIES as usize]);

                let instant = Instant::now();
                server_app.update();
                elapsed += instant.elapsed();

                client_app.update();
                assert_eq!(client_app.world.entities().len(), ENTITIES);
            }

            elapsed
        })
    });

    c.bench_function("entities receive", |b| {
        b.iter_custom(|iter| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iter {
                let mut server_app = App::new();
                let mut client_app = App::new();
                for app in [&mut server_app, &mut client_app] {
                    setup_app(app);
                }
                common::connect(&mut server_app, &mut client_app);

                server_app
                    .world
                    .spawn_batch([(Replication, DummyComponent); ENTITIES as usize]);

                server_app.update();

                let instant = Instant::now();
                client_app.update();
                elapsed += instant.elapsed();
                assert_eq!(client_app.world.entities().len(), ENTITIES);
            }

            elapsed
        })
    });
}

fn setup_app(app: &mut App) {
    app.add_plugins((
        MinimalPlugins,
        ReplicationPlugins.set(ServerPlugin::new(TickPolicy::Manual)),
    ))
    .replicate::<DummyComponent>();

    // TODO 0.12: Probably won't be needed since `multi-threaded` feature will be disabled by default.
    let labels = app.world.resource::<MainScheduleOrder>().labels.clone();
    for label in labels {
        app.edit_schedule(label, |schedule| {
            schedule.set_executor_kind(ExecutorKind::SingleThreaded);
        });
    }
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(50);
    targets = replication
}
criterion_main!(benches);
