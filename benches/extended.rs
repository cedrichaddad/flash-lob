//! Extended Benchmark Suite - More comprehensive latency measurements.
//!
//! Includes:
//! - P99 latency estimation via histogram
//! - Matching across multiple price levels
//! - Book depth impact on performance
//! - Memory allocation pressure tests

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use flash_lob::{Engine, Command, PlaceOrder, CancelOrder, Side, OrderType};
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use std::time::Instant;

/// Benchmark: Match across multiple price levels
fn bench_multi_level_match(c: &mut Criterion) {
    let mut group = c.benchmark_group("multi_level_match");
    
    for levels in [1, 5, 10, 20].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(levels),
            levels,
            |b, &levels| {
                let mut engine = Engine::new(100_000);
                engine.warm_up();
                
                // Pre-populate with orders across multiple levels
                for i in 0..levels {
                    for j in 0..10 {
                        engine.process_command(Command::Place(PlaceOrder {
                            order_id: (i * 10 + j) as u64,
                            user_id: 1,
                            side: Side::Ask,
                            price: 10000 + i as u64 * 10,
                            qty: 10, order_type: OrderType::Limit,
                        }));
                    }
                }
                
                let mut order_id = 1000u64;
                
                b.iter(|| {
                    order_id += 1;
                    let result = engine.process_command(Command::Place(PlaceOrder {
                        order_id,
                        user_id: 2,
                        side: Side::Bid,
                        price: 10000 + (levels as u64 - 1) * 10,
                        qty: levels as u32 * 10, // Match one order per level
                        order_type: OrderType::Limit,
                    }));
                    
                    // Replenish
                    for i in 0..levels {
                        engine.process_command(Command::Place(PlaceOrder {
                            order_id: order_id + 1000 + i as u64,
                            user_id: 1,
                            side: Side::Ask,
                            price: 10000 + i as u64 * 10,
                            qty: 10, order_type: OrderType::Limit,
                        }));
                    }
                    
                    black_box(result)
                })
            },
        );
    }
    
    group.finish();
}

/// Benchmark: Impact of book depth on order placement
fn bench_book_depth_impact(c: &mut Criterion) {
    let mut group = c.benchmark_group("book_depth_place");
    
    for depth in [100, 1_000, 10_000, 50_000].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(depth),
            depth,
            |b, &depth| {
                let mut engine = Engine::new(100_000);
                engine.warm_up();
                
                // Pre-populate book
                for i in 0..depth {
                    engine.process_command(Command::Place(PlaceOrder {
                        order_id: i as u64,
                        user_id: 1,
                        side: if i % 2 == 0 { Side::Bid } else { Side::Ask },
                        price: 9000 + (i % 100) as u64 * 10,
                        qty: 100, order_type: OrderType::Limit,
                    }));
                }
                
                let mut order_id = depth as u64;
                
                b.iter(|| {
                    order_id += 1;
                    // Place a non-matching order
                    black_box(engine.process_command(Command::Place(PlaceOrder {
                        order_id,
                        user_id: 2,
                        side: Side::Bid,
                        price: 8000, // Won't match
                        qty: 100, order_type: OrderType::Limit,
                    })))
                })
            },
        );
    }
    
    group.finish();
}

/// Benchmark: Cancel performance with varying book sizes
fn bench_cancel_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("cancel_scaling");
    
    for book_size in [100, 1_000, 10_000, 50_000].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(book_size),
            book_size,
            |b, &book_size| {
                let mut engine = Engine::new(100_000);
                engine.warm_up();
                
                // Pre-populate
                for i in 0..book_size {
                    engine.process_command(Command::Place(PlaceOrder {
                        order_id: i as u64,
                        user_id: 1,
                        side: if i % 2 == 0 { Side::Bid } else { Side::Ask },
                        price: 9000 + (i % 200) as u64 * 10,
                        qty: 100, order_type: OrderType::Limit,
                    }));
                }
                
                let mut rng = ChaCha8Rng::seed_from_u64(0xDEADBEEF);
                let mut cancel_id = 0u64;
                let mut next_id = book_size as u64;
                
                b.iter(|| {
                    // Cancel random order
                    let result = engine.process_command(Command::Cancel(CancelOrder {
                        order_id: cancel_id,
                    }));
                    
                    // Replenish
                    engine.process_command(Command::Place(PlaceOrder {
                        order_id: next_id,
                        user_id: 1,
                        side: if cancel_id % 2 == 0 { Side::Bid } else { Side::Ask },
                        price: 9000 + (cancel_id % 200) * 10,
                        qty: 100, order_type: OrderType::Limit,
                    }));
                    
                    cancel_id = next_id;
                    next_id += 1;
                    
                    black_box(result)
                })
            },
        );
    }
    
    group.finish();
}

/// Benchmark: Modify order performance
fn bench_modify_order(c: &mut Criterion) {
    let mut group = c.benchmark_group("modify_order");
    
    for book_size in [100, 1_000, 10_000].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(book_size),
            book_size,
            |b, &book_size| {
                let mut engine = Engine::new(100_000);
                engine.warm_up();
                
                // Pre-populate
                for i in 0..book_size {
                    engine.process_command(Command::Place(PlaceOrder {
                        order_id: i as u64,
                        user_id: 1,
                        side: if i % 2 == 0 { Side::Bid } else { Side::Ask },
                        price: 9000 + (i % 100) as u64 * 10,
                        qty: 100, order_type: OrderType::Limit,
                    }));
                }
                
                let mut modify_id = 0u64;
                let mut new_id = book_size as u64;
                
                b.iter(|| {
                    let result = engine.process_command(Command::Modify(
                        flash_lob::ModifyOrder {
                            order_id: modify_id,
                            new_order_id: new_id,
                            new_price: 9500,
                            new_qty: 150,
                        }
                    ));
                    
                    modify_id = new_id;
                    new_id += 1;
                    
                    black_box(result)
                })
            },
        );
    }
    
    group.finish();
}

/// Benchmark: High-frequency realistic workload
fn bench_realistic_hft(c: &mut Criterion) {
    let mut group = c.benchmark_group("realistic_hft");
    
    // Simulate HFT workload: tight spread, many cancels
    group.bench_function("tight_spread_workload", |b| {
        let mut engine = Engine::new(100_000);
        engine.warm_up();
        
        let mut rng = ChaCha8Rng::seed_from_u64(0xCAFEBABE);
        let mut order_id = 0u64;
        
        // Pre-populate with tight spread
        for i in 0..500 {
            engine.process_command(Command::Place(PlaceOrder {
                order_id: i,
                user_id: 1,
                side: Side::Bid,
                price: 9990 + (i % 10) as u64, // 9990-9999
                qty: 100, order_type: OrderType::Limit,
            }));
            engine.process_command(Command::Place(PlaceOrder {
                order_id: 500 + i,
                user_id: 1,
                side: Side::Ask,
                price: 10001 + (i % 10) as u64, // 10001-10010
                qty: 100, order_type: OrderType::Limit,
            }));
        }
        
        order_id = 1000;
        
        b.iter(|| {
            let op = rng.gen_range(0..100);
            
            let result = if op < 40 {
                // 40% place bid
                order_id += 1;
                engine.process_command(Command::Place(PlaceOrder {
                    order_id,
                    user_id: rng.gen_range(1..100),
                    side: Side::Bid,
                    price: 9990 + rng.gen_range(0..10),
                    qty: rng.gen_range(10..200), order_type: OrderType::Limit,
                }))
            } else if op < 80 {
                // 40% place ask
                order_id += 1;
                engine.process_command(Command::Place(PlaceOrder {
                    order_id,
                    user_id: rng.gen_range(1..100),
                    side: Side::Ask,
                    price: 10001 + rng.gen_range(0..10),
                    qty: rng.gen_range(10..200), order_type: OrderType::Limit,
                }))
            } else {
                // 20% cancel
                let cancel_id = rng.gen_range(0..order_id);
                engine.process_command(Command::Cancel(CancelOrder {
                    order_id: cancel_id,
                }))
            };
            
            black_box(result)
        })
    });
    
    group.finish();
}

/// Benchmark: Warm vs cold cache performance
fn bench_cache_effects(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_effects");
    
    // Small book (fits in L2 cache)
    group.bench_function("small_book_100", |b| {
        let mut engine = Engine::new(1000);
        engine.warm_up();
        
        for i in 0..100 {
            engine.process_command(Command::Place(PlaceOrder {
                order_id: i,
                user_id: 1,
                side: Side::Ask,
                price: 10000,
                qty: 100, order_type: OrderType::Limit,
            }));
        }
        
        let mut order_id = 1000u64;
        
        b.iter(|| {
            order_id += 1;
            let result = engine.process_command(Command::Place(PlaceOrder {
                order_id,
                user_id: 2,
                side: Side::Bid,
                price: 10000,
                qty: 100, order_type: OrderType::Limit,
            }));
            
            // Replenish
            engine.process_command(Command::Place(PlaceOrder {
                order_id: order_id + 100000,
                user_id: 1,
                side: Side::Ask,
                price: 10000,
                qty: 100, order_type: OrderType::Limit,
            }));
            
            black_box(result)
        })
    });
    
    // Large book (likely spills to L3/RAM)
    group.bench_function("large_book_50k", |b| {
        let mut engine = Engine::new(100_000);
        engine.warm_up();
        
        for i in 0..50_000 {
            engine.process_command(Command::Place(PlaceOrder {
                order_id: i,
                user_id: 1,
                side: if i % 2 == 0 { Side::Ask } else { Side::Bid },
                price: 9000 + (i % 1000) as u64,
                qty: 100, order_type: OrderType::Limit,
            }));
        }
        
        let mut order_id = 100_000u64;
        
        b.iter(|| {
            order_id += 1;
            let result = engine.process_command(Command::Place(PlaceOrder {
                order_id,
                user_id: 2,
                side: Side::Bid,
                price: 9500, // Somewhere in the middle
                qty: 100, order_type: OrderType::Limit,
            }));
            
            black_box(result)
        })
    });
    
    group.finish();
}

/// Benchmark: Throughput with batch processing
fn bench_batch_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_throughput");
    
    for batch_size in [100, 1_000, 10_000].iter() {
        group.throughput(Throughput::Elements(*batch_size as u64));
        
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            batch_size,
            |b, &batch_size| {
                let mut engine = Engine::new(100_000);
                engine.warm_up();
                
                let mut rng = ChaCha8Rng::seed_from_u64(0x12345678);
                
                b.iter(|| {
                    for i in 0..batch_size {
                        let cmd = Command::Place(PlaceOrder {
                            order_id: i as u64,
                            user_id: rng.gen_range(1..100),
                            side: if rng.gen_bool(0.5) { Side::Bid } else { Side::Ask },
                            price: rng.gen_range(9900..10100) * 100,
                            qty: rng.gen_range(1..500), order_type: OrderType::Limit,
                        });
                        black_box(engine.process_command(cmd));
                    }
                    engine.matcher.book.clear();
                })
            },
        );
    }
    
    group.finish();
}

criterion_group!(
    extended_benches,
    bench_multi_level_match,
    bench_book_depth_impact,
    bench_cancel_scaling,
    bench_modify_order,
    bench_realistic_hft,
    bench_cache_effects,
    bench_batch_throughput,
);

criterion_main!(extended_benches);
