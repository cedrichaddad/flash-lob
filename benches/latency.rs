//! Benchmark harness using Criterion for latency measurement.
//!
//! Measures:
//! - Place order (no match)
//! - Place order (full match)
//! - Cancel order
//! - Mixed workload

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use flash_lob::{Engine, Command, PlaceOrder, CancelOrder, Side, OrderType};
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;

/// Generate a random place order command
fn random_place(rng: &mut ChaCha8Rng, order_id: u64) -> Command {
    Command::Place(PlaceOrder {
        order_id,
        user_id: rng.gen_range(1..1000),
        side: if rng.gen_bool(0.5) { Side::Bid } else { Side::Ask },
        price: rng.gen_range(9900..10100) * 100, // 990.00 to 1010.00
        qty: rng.gen_range(1..1000),
        order_type: OrderType::Limit,
    })
}

/// Benchmark: Place order that rests (no matching)
fn bench_place_no_match(c: &mut Criterion) {
    let mut engine = Engine::new(100_000);
    engine.warm_up();
    
    let mut order_id = 0u64;
    
    c.bench_function("place_no_match", |b| {
        b.iter(|| {
            order_id += 1;
            let cmd = Command::Place(PlaceOrder {
                order_id,
                user_id: 1,
                side: Side::Bid,
                price: 9000, // Below any asks
                qty: 100,
                order_type: OrderType::Limit,
            });
            black_box(engine.process_command(cmd))
        })
    });
}

/// Benchmark: Place order that fully matches
fn bench_place_full_match(c: &mut Criterion) {
    let mut group = c.benchmark_group("place_full_match");
    
    for depth in [1, 10, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(depth), depth, |b, &depth| {
            let mut engine = Engine::new(100_000);
            engine.warm_up();
            
            // Pre-populate with resting orders
            for i in 0..depth {
                engine.process_command(Command::Place(PlaceOrder {
                    order_id: i as u64,
                    user_id: 1,
                    side: Side::Ask,
                    price: 10000,
                    qty: 100,
                    order_type: OrderType::Limit,
                }));
            }
            
            let mut order_id = 1000u64;
            
            b.iter(|| {
                order_id += 1;
                // Place matching bid
                let cmd = Command::Place(PlaceOrder {
                    order_id,
                    user_id: 2,
                    side: Side::Bid,
                    price: 10000,
                    qty: 100,
                    order_type: OrderType::Limit,
                });
                let result = engine.process_command(cmd);
                
                // Replenish the matched order
                engine.process_command(Command::Place(PlaceOrder {
                    order_id: order_id + 1_000_000,
                    user_id: 1,
                    side: Side::Ask,
                    price: 10000,
                    qty: 100,
                    order_type: OrderType::Limit,
                }));
                
                black_box(result)
            })
        });
    }
    
    group.finish();
}

/// Benchmark: Cancel order
fn bench_cancel(c: &mut Criterion) {
    let mut group = c.benchmark_group("cancel");
    
    for book_size in [100, 1000, 10000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(book_size), book_size, |b, &book_size| {
            let mut engine = Engine::new(100_000);
            engine.warm_up();
            
            // Pre-populate book
            for i in 0..book_size {
                engine.process_command(Command::Place(PlaceOrder {
                    order_id: i as u64,
                    user_id: 1,
                    side: if i % 2 == 0 { Side::Bid } else { Side::Ask },
                    price: 9000 + (i % 100) as u64 * 10,
                    qty: 100,
                    order_type: OrderType::Limit,
                }));
            }
            
            let mut cancel_id = 0u64;
            let mut next_order_id = book_size as u64;
            
            b.iter(|| {
                // Cancel an order
                let result = engine.process_command(Command::Cancel(CancelOrder {
                    order_id: cancel_id,
                }));
                
                // Replenish
                engine.process_command(Command::Place(PlaceOrder {
                    order_id: next_order_id,
                    user_id: 1,
                    side: if cancel_id % 2 == 0 { Side::Bid } else { Side::Ask },
                    price: 9000 + (cancel_id % 100) * 10,
                    qty: 100,
                    order_type: OrderType::Limit,
                }));
                
                cancel_id = next_order_id;
                next_order_id += 1;
                
                black_box(result)
            })
        });
    }
    
    group.finish();
}

/// Benchmark: Mixed workload (realistic trading scenario)
fn bench_mixed_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("mixed_workload");
    
    // 70% place, 30% cancel
    group.bench_function("70_place_30_cancel", |b| {
        let mut engine = Engine::new(100_000);
        engine.warm_up();
        
        let mut rng = ChaCha8Rng::seed_from_u64(0xDEADBEEF);
        let mut order_id = 0u64;
        
        // Pre-populate
        for _ in 0..1000 {
            order_id += 1;
            engine.process_command(random_place(&mut rng, order_id));
        }
        
        b.iter(|| {
            if rng.gen_bool(0.7) {
                // Place
                order_id += 1;
                black_box(engine.process_command(random_place(&mut rng, order_id)))
            } else {
                // Cancel (random existing order)
                let cancel_id = rng.gen_range(1..=order_id);
                black_box(engine.process_command(Command::Cancel(CancelOrder {
                    order_id: cancel_id,
                })))
            }
        })
    });
    
    group.finish();
}

/// Benchmark: Throughput (orders per second)
fn bench_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("throughput");
    group.throughput(criterion::Throughput::Elements(1000));
    
    group.bench_function("1000_orders", |b| {
        let mut engine = Engine::new(100_000);
        engine.warm_up();
        
        let mut rng = ChaCha8Rng::seed_from_u64(0xCAFEBABE);
        
        b.iter(|| {
            for i in 0..1000 {
                let cmd = random_place(&mut rng, i);
                black_box(engine.process_command(cmd));
            }
            engine.matcher.book.clear();
        })
    });
    
    group.finish();
}

criterion_group!(
    benches,
    bench_place_no_match,
    bench_place_full_match,
    bench_cancel,
    bench_mixed_workload,
    bench_throughput,
);

criterion_main!(benches);
