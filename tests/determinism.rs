//! Determinism Test - Golden Master verification.
//!
//! Verifies that the engine produces identical results across runs
//! when given the same input sequence.

use flash_lob::{Engine, Command, PlaceOrder, CancelOrder, Side};
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Generate a deterministic sequence of commands
fn generate_commands(seed: u64, count: usize) -> Vec<Command> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut commands = Vec::with_capacity(count);
    let mut active_orders: Vec<u64> = Vec::new();
    let mut next_order_id = 1u64;
    
    for _ in 0..count {
        // 70% place, 30% cancel
        if active_orders.is_empty() || rng.gen_bool(0.7) {
            // Place order
            let order_id = next_order_id;
            next_order_id += 1;
            
            commands.push(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
                order_id,
                user_id: rng.gen_range(1..100),
                side: if rng.gen_bool(0.5) { Side::Bid } else { Side::Ask },
                price: rng.gen_range(9500..10500) * 100, // 950.00 to 1050.00
                qty: rng.gen_range(1..500),
            }));
            
            active_orders.push(order_id);
        } else {
            // Cancel random active order
            let idx = rng.gen_range(0..active_orders.len());
            let order_id = active_orders.swap_remove(idx);
            
            commands.push(Command::Cancel(CancelOrder { order_id }));
        }
    }
    
    commands
}

/// Compute a hash of all output events
fn hash_events(events: &[flash_lob::OutputEvent]) -> u64 {
    let mut hasher = DefaultHasher::new();
    
    for event in events {
        match event {
            flash_lob::OutputEvent::Trade(t) => {
                "Trade".hash(&mut hasher);
                t.price.hash(&mut hasher);
                t.qty.hash(&mut hasher);
                t.maker_order_id.hash(&mut hasher);
                t.taker_order_id.hash(&mut hasher);
            }
            flash_lob::OutputEvent::Accepted(a) => {
                "Accepted".hash(&mut hasher);
                a.order_id.hash(&mut hasher);
                a.price.hash(&mut hasher);
                a.qty.hash(&mut hasher);
            }
            flash_lob::OutputEvent::Canceled(c) => {
                "Canceled".hash(&mut hasher);
                c.order_id.hash(&mut hasher);
                c.canceled_qty.hash(&mut hasher);
            }
            flash_lob::OutputEvent::BookDelta(b) => {
                "BookDelta".hash(&mut hasher);
                b.price.hash(&mut hasher);
                b.new_qty.hash(&mut hasher);
                b.new_count.hash(&mut hasher);
            }
            flash_lob::OutputEvent::Rejected(r) => {
                "Rejected".hash(&mut hasher);
                r.order_id.hash(&mut hasher);
            }
        }
    }
    
    hasher.finish()
}

/// Run the engine with a command sequence and return hashes
fn run_engine(commands: &[Command]) -> (u64, u64) {
    let mut engine = Engine::new(100_000);
    let mut all_events = Vec::new();
    
    for cmd in commands {
        let events = engine.process_command(*cmd);
        all_events.extend(events);
    }
    
    let event_hash = hash_events(&all_events);
    let state_hash = engine.state_hash();
    
    (event_hash, state_hash)
}

#[test]
fn test_determinism_small() {
    const SEED: u64 = 0xDEADBEEF;
    const COUNT: usize = 1000;
    const RUNS: usize = 10;
    
    let commands = generate_commands(SEED, COUNT);
    
    // Run multiple times and verify identical results
    let (first_event_hash, first_state_hash) = run_engine(&commands);
    
    for run in 1..RUNS {
        let (event_hash, state_hash) = run_engine(&commands);
        
        assert_eq!(
            event_hash, first_event_hash,
            "Event hash mismatch on run {}", run
        );
        assert_eq!(
            state_hash, first_state_hash,
            "State hash mismatch on run {}", run
        );
    }
    
    println!("Determinism test passed!");
    println!("  Commands: {}", COUNT);
    println!("  Runs: {}", RUNS);
    println!("  Event hash: {:#018x}", first_event_hash);
    println!("  State hash: {:#018x}", first_state_hash);
}

#[test]
fn test_determinism_large() {
    const SEED: u64 = 0xCAFEBABE;
    const COUNT: usize = 100_000;
    const RUNS: usize = 3;
    
    let commands = generate_commands(SEED, COUNT);
    
    let (first_event_hash, first_state_hash) = run_engine(&commands);
    
    for run in 1..RUNS {
        let (event_hash, state_hash) = run_engine(&commands);
        
        assert_eq!(event_hash, first_event_hash, "Event hash mismatch on run {}", run);
        assert_eq!(state_hash, first_state_hash, "State hash mismatch on run {}", run);
    }
    
    println!("Large determinism test passed!");
    println!("  Commands: {}", COUNT);
    println!("  Event hash: {:#018x}", first_event_hash);
    println!("  State hash: {:#018x}", first_state_hash);
}

#[test]
fn test_different_seeds_produce_different_results() {
    let commands1 = generate_commands(1, 1000);
    let commands2 = generate_commands(2, 1000);
    
    let (hash1, _) = run_engine(&commands1);
    let (hash2, _) = run_engine(&commands2);
    
    assert_ne!(hash1, hash2, "Different seeds should produce different results");
}
