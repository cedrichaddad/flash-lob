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
    
    println!("Running {} iterations...", ITERATIONS);
    
    let mut order_id = 0;
    let mut total_duration = std::time::Duration::new(0, 0);
    
    for _ in 0..ITERATIONS {
        order_id += 1;
        
        let cmd = Command::Place(PlaceOrder {
            order_id,
            user_id: 1,
            side: if order_id % 2 == 0 { Side::Bid } else { Side::Ask },
            price: 10000 + (order_id % 100),
            qty: 10,
            order_type: OrderType::Limit,
        });
        
        // Critical measurement section
        let start = Instant::now();
        
        // Use black_box to prevent compiler optimization
        std::hint::black_box(engine.process_command(cmd));
        
        let elapsed = start.elapsed();
        
        // Record nanoseconds
        // We use check_add to avoid panics on outliers, though 100us max should be enough for "flash" lob
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
