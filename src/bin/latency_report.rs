use flash_lob::{Engine, Command, PlaceOrder, Side, OrderType};
use hdrhistogram::Histogram;
use std::time::Instant;

fn main() {
    println!("Preparing Latency Benchmark...");
    
    // Setup
    let mut engine = Engine::new(100_000);
    engine.warm_up();
    
    let mut histogram = Histogram::<u64>::new_with_bounds(1, 100_000, 3).unwrap();
    
    const ITERATIONS: u64 = 1_000_000;
    const BUFFER_SIZE: usize = 10_000;
    
    // 1. Pre-generate commands to avoid RNG/Alloc overhead during partial checks
    println!("Pre-generating {} commands...", BUFFER_SIZE);
    let mut commands = Vec::with_capacity(BUFFER_SIZE);
    for i in 0..BUFFER_SIZE {
        let order_id = (i + 1) as u64;
        commands.push(Command::Place(PlaceOrder {
            order_id,
            user_id: 1,
            side: if i % 2 == 0 { Side::Bid } else { Side::Ask },
            price: 10000 + (order_id % 100),
            qty: 10,
            order_type: OrderType::Limit,
        }));
    }
    
    // 2. Execution Warmup (Train Branch Predictor)
    println!("Warming up branch predictor ({} ops)...", BUFFER_SIZE);
    for cmd in commands.iter() {
        // Clone to keep the command for the real run? 
        // No, we need fresh commands or reset.
        // Actually, reusing commands with same ID might be weird if checking for duplicates,
        // but engine doesn't check duplicates strictly in this microbenchmark (it's HashMap insert).
        // To be safe, let's just run some dummy commands.
        let warm_cmd = cmd.clone();
        std::hint::black_box(engine.process_command(warm_cmd));
    }
    
    // Reset engine for clean run? 
    // Ideally yes, but arena reuse is part of the perf. 
    // Let's keep it hot.
    
    println!("Running {} iterations...", ITERATIONS);
    
    let mut total_duration = std::time::Duration::new(0, 0);
    
    let mut command_ring_buf = commands.into_iter().cycle();
    
    for _ in 0..ITERATIONS {
        let cmd = command_ring_buf.next().unwrap();
        // Modification to order_id to simulate new orders if needed?
        // Cloning is cheap for u64s/structs.
        // But `Command` owns data.
        // To avoid clone overhead in the loop, we should ideally have the vector ready.
        // But `process_command` takes ownership `Command`.
        // So we MUST clone or generate.
        // `PlaceOrder` is Copy? No, it has `OrderType` which is Copy.
        // `PlaceOrder` should be `Copy` ideally. Let's assume Clone is cheap (memcpy).
        
        let exec_cmd = cmd.clone();
        
        // Critical measurement section
        let start = Instant::now();
        
        // Use black_box to prevent compiler optimization
        std::hint::black_box(engine.process_command(exec_cmd));
        
        let elapsed = start.elapsed();
        
        // Record nanoseconds
        histogram.record(elapsed.as_nanos() as u64).unwrap_or(());
        total_duration += elapsed;
    }
    
    println!("\n=== Latency Report (ns) ===");
    println!("Total Ops:  {}", ITERATIONS);
    println!("Throughput: {:.2} ops/sec", ITERATIONS as f64 / total_duration.as_secs_f64());
    println!("---------------------------");
    println!("Min:    {:6} ns", histogram.min());
    println!("P50:    {:6} ns", histogram.value_at_quantile(0.50));
    println!("P90:    {:6} ns", histogram.value_at_quantile(0.90));
    println!("P99:    {:6} ns", histogram.value_at_quantile(0.99));
    println!("P99.9:  {:6} ns", histogram.value_at_quantile(0.999));
    println!("P99.99: {:6} ns", histogram.value_at_quantile(0.9999));
    println!("Max:    {:6} ns", histogram.max());
    println!("---------------------------");
    
    // Quick ASCII histogram
    println!("\nDistribution:");
    for v in histogram.iter_log(100_000, 2.0) {
        let count = v.count_at_value();
        if count > 0 {
            println!("{:6} ns - {:6} ns: {:10} count", 
                v.value_iterated_to(), // approximate bucket value
                v.value_iterated_to(), 
                count
            );
        }
    }
}
