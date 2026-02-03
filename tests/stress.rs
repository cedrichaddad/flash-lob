//! Stress Tests - Push the engine to its limits.
//!
//! These tests verify correctness under extreme conditions:
//! - Near-capacity operation
//! - High contention at single price levels
//! - Rapid order churn
//! - Maximum values for prices and quantities

use flash_lob::{Engine, Command, PlaceOrder, CancelOrder, Side, OutputEvent, OrderType};
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;

// ============================================================================
// Capacity Stress Tests
// ============================================================================

#[test]
fn test_near_capacity_operation() {
    const CAPACITY: u32 = 10_000;
    let mut engine = Engine::new(CAPACITY);
    
    // Fill to 95% capacity
    let target_orders = (CAPACITY as f64 * 0.95) as u64;
    
    for i in 0..target_orders {
        // Use non-overlapping prices: bids 8000-8999, asks 10000-10999
        let (side, price) = if i % 2 == 0 {
            (Side::Bid, 8000 + (i % 100) * 10)
        } else {
            (Side::Ask, 10000 + (i % 100) * 10)
        };
        let events = engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
            order_id: i,
            user_id: 1,
            side,
            price,
            qty: 100,
        }));
        
        // Verify order was accepted (not rejected due to arena full)
        assert!(
            events.iter().any(|e| matches!(e, OutputEvent::Accepted(_))),
            "Order {} should be accepted, got {:?}", i, events
        );
    }
    
    assert_eq!(engine.order_count(), target_orders as usize);
}

#[test]
fn test_arena_full_rejection() {
    const CAPACITY: u32 = 100;
    let mut engine = Engine::new(CAPACITY);
    
    // Fill arena completely
    for i in 0..CAPACITY as u64 {
        engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
            order_id: i,
            user_id: 1,
            side: Side::Bid,
            price: 9000 + i * 10,
            qty: 100,
        }));
    }
    
    // Next order should be rejected
    let events = engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: CAPACITY as u64,
        user_id: 1,
        side: Side::Bid,
        price: 10000,
        qty: 100,
    }));
    
    assert!(
        events.iter().any(|e| matches!(e, OutputEvent::Rejected(_))),
        "Order should be rejected when arena is full"
    );
}

#[test]
fn test_arena_reuse_after_cancel() {
    const CAPACITY: u32 = 100;
    let mut engine = Engine::new(CAPACITY);
    
    // Fill arena
    for i in 0..CAPACITY as u64 {
        engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
            order_id: i,
            user_id: 1,
            side: Side::Bid,
            price: 9000,
            qty: 100,
        }));
    }
    
    // Cancel one order
    engine.process_command(Command::Cancel(CancelOrder { order_id: 50 }));
    
    // Now we can add one more
    let events = engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 1000,
        user_id: 1,
        side: Side::Bid,
        price: 9000,
        qty: 100,
    }));
    
    assert!(
        events.iter().any(|e| matches!(e, OutputEvent::Accepted(_))),
        "Should be able to add order after cancel frees slot"
    );
}

// ============================================================================
// High Contention Tests
// ============================================================================

#[test]
fn test_single_price_level_contention() {
    let mut engine = Engine::new(10_000);
    const ORDERS_PER_SIDE: u64 = 1000;
    
    // Add many orders at the same price
    for i in 0..ORDERS_PER_SIDE {
        engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
            order_id: i,
            user_id: i % 100,
            side: Side::Ask,
            price: 10000, // All at same price
            qty: 100,
        }));
    }
    
    // Verify all are tracked
    assert_eq!(engine.order_count(), ORDERS_PER_SIDE as usize);
    
    // Match through all of them
    let events = engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: ORDERS_PER_SIDE,
        user_id: 999,
        side: Side::Bid,
        price: 10000,
        qty: (ORDERS_PER_SIDE * 100) as u32, // Match all
    }));
    
    let trade_count = events.iter()
        .filter(|e| matches!(e, OutputEvent::Trade(_)))
        .count();
    
    assert_eq!(trade_count, ORDERS_PER_SIDE as usize, "Should have {} trades", ORDERS_PER_SIDE);
    assert_eq!(engine.order_count(), 0, "Book should be empty after matching all");
}

#[test]
fn test_fifo_priority_under_contention() {
    let mut engine = Engine::new(1000);
    
    // Add 100 orders at same price
    for i in 0..100u64 {
        engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
            order_id: i,
            user_id: i,
            side: Side::Ask,
            price: 10000,
            qty: 10,
        }));
    }
    
    // Match 50 orders worth
    let events = engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 1000,
        user_id: 999,
        side: Side::Bid,
        price: 10000,
        qty: 500, // 50 orders @ 10 qty each
    }));
    
    // Verify FIFO order
    let trades: Vec<_> = events.iter()
        .filter_map(|e| if let OutputEvent::Trade(t) = e { Some(t.maker_order_id) } else { None })
        .collect();
    
    assert_eq!(trades.len(), 50);
    for (i, &maker_id) in trades.iter().enumerate() {
        assert_eq!(maker_id, i as u64, "Trade {} should match order {}", i, i);
    }
}

// ============================================================================
// Rapid Churn Tests
// ============================================================================

#[test]
fn test_rapid_add_cancel_cycles() {
    let mut engine = Engine::new(1000);
    const CYCLES: usize = 10_000;
    
    for cycle in 0..CYCLES {
        let order_id = cycle as u64;
        
        // Add
        let add_events = engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
            order_id,
            user_id: 1,
            side: if cycle % 2 == 0 { Side::Bid } else { Side::Ask },
            price: 10000,
            qty: 100,
        }));
        
        assert!(add_events.iter().any(|e| matches!(e, OutputEvent::Accepted(_))));
        
        // Cancel
        let cancel_events = engine.process_command(Command::Cancel(CancelOrder { order_id }));
        
        assert!(cancel_events.iter().any(|e| matches!(e, OutputEvent::Canceled(_))));
    }
    
    assert_eq!(engine.order_count(), 0, "All orders should be canceled");
}

#[test]
fn test_rapid_match_cycles() {
    let mut engine = Engine::new(10_000);
    const CYCLES: usize = 5_000;
    
    let mut total_trades = 0;
    
    for cycle in 0..CYCLES {
        // Place ask
        engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
            order_id: cycle as u64 * 2,
            user_id: 1,
            side: Side::Ask,
            price: 10000,
            qty: 100,
        }));
        
        // Place matching bid
        let events = engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
            order_id: cycle as u64 * 2 + 1,
            user_id: 2,
            side: Side::Bid,
            price: 10000,
            qty: 100,
        }));
        
        total_trades += events.iter()
            .filter(|e| matches!(e, OutputEvent::Trade(_)))
            .count();
    }
    
    assert_eq!(total_trades, CYCLES, "Should have {} trades", CYCLES);
    assert_eq!(engine.order_count(), 0, "Book should be empty");
}

// ============================================================================
// Edge Case Tests
// ============================================================================

#[test]
fn test_zero_price() {
    let mut engine = Engine::new(1000);
    
    // Price of 0 should work (might represent free assets)
    let events = engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 1,
        user_id: 1,
        side: Side::Bid,
        price: 0,
        qty: 100,
    }));
    
    assert!(events.iter().any(|e| matches!(e, OutputEvent::Accepted(_))));
    assert_eq!(engine.best_bid(), Some(0));
}

#[test]
fn test_max_price() {
    let mut engine = Engine::new(1000);
    
    let events = engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 1,
        user_id: 1,
        side: Side::Ask,
        price: u64::MAX - 1, // Avoid overflow issues
        qty: 100,
    }));
    
    assert!(events.iter().any(|e| matches!(e, OutputEvent::Accepted(_))));
    assert_eq!(engine.best_ask(), Some(u64::MAX - 1));
}

#[test]
fn test_max_quantity() {
    let mut engine = Engine::new(1000);
    
    let events = engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 1,
        user_id: 1,
        side: Side::Bid,
        price: 10000,
        qty: u32::MAX,
    }));
    
    assert!(events.iter().any(|e| matches!(e, OutputEvent::Accepted(_))));
}

#[test]
fn test_quantity_one() {
    let mut engine = Engine::new(1000);
    
    let events = engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 1,
        user_id: 1,
        side: Side::Bid,
        price: 10000,
        qty: 1,
    }));
    
    assert!(events.iter().any(|e| matches!(e, OutputEvent::Accepted(_))));
}

#[test]
fn test_many_price_levels() {
    let mut engine = Engine::new(100_000);
    const LEVELS: u64 = 10_000;
    
    // Create many sparse price levels
    for i in 0..LEVELS {
        engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
            order_id: i,
            user_id: 1,
            side: Side::Bid,
            price: i * 1000, // Very sparse
            qty: 100,
        }));
    }
    
    assert_eq!(engine.order_count(), LEVELS as usize);
    assert_eq!(engine.best_bid(), Some((LEVELS - 1) * 1000));
}

// ============================================================================
// Cancel Edge Cases
// ============================================================================

#[test]
fn test_double_cancel() {
    let mut engine = Engine::new(1000);
    
    engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 1,
        user_id: 1,
        side: Side::Bid,
        price: 10000,
        qty: 100,
    }));
    
    // First cancel
    let events1 = engine.process_command(Command::Cancel(CancelOrder { order_id: 1 }));
    assert!(events1.iter().any(|e| matches!(e, OutputEvent::Canceled(_))));
    
    // Second cancel should be rejected
    let events2 = engine.process_command(Command::Cancel(CancelOrder { order_id: 1 }));
    assert!(events2.iter().any(|e| matches!(e, OutputEvent::Rejected(_))));
}

#[test]
fn test_cancel_during_partial_fill() {
    let mut engine = Engine::new(1000);
    
    // Place large resting order
    engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 1,
        user_id: 1,
        side: Side::Ask,
        price: 10000,
        qty: 1000,
    }));
    
    // Partially fill it
    engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 2,
        user_id: 2,
        side: Side::Bid,
        price: 10000,
        qty: 300,
    }));
    
    // Cancel remaining
    let events = engine.process_command(Command::Cancel(CancelOrder { order_id: 1 }));
    
    let canceled = events.iter()
        .find_map(|e| if let OutputEvent::Canceled(c) = e { Some(c.canceled_qty) } else { None });
    
    assert_eq!(canceled, Some(700), "Should cancel remaining 700 qty");
}

// ============================================================================
// ModifyOrder Tests
// ============================================================================

#[test]
fn test_modify_order_basic() {
    let mut engine = Engine::new(1000);
    
    // Place original order
    engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 1,
        user_id: 100,
        side: Side::Bid,
        price: 10000,
        qty: 100,
    }));
    
    assert_eq!(engine.best_bid(), Some(10000));
    
    // Modify to new price
    let events = engine.process_command(Command::Modify(flash_lob::ModifyOrder {
        order_id: 1,
        new_order_id: 2,
        new_price: 10500,
        new_qty: 200,
    }));
    
    // Should have cancel + accept events
    assert!(events.iter().any(|e| matches!(e, OutputEvent::Canceled(_))));
    assert!(events.iter().any(|e| matches!(e, OutputEvent::Accepted(_))));
    
    assert_eq!(engine.best_bid(), Some(10500));
    assert_eq!(engine.order_count(), 1);
}

#[test]
fn test_modify_preserves_side() {
    let mut engine = Engine::new(1000);
    
    // Place ask
    engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 1,
        user_id: 100,
        side: Side::Ask,
        price: 10000,
        qty: 100,
    }));
    
    assert_eq!(engine.best_ask(), Some(10000));
    assert_eq!(engine.best_bid(), None);
    
    // Modify
    engine.process_command(Command::Modify(flash_lob::ModifyOrder {
        order_id: 1,
        new_order_id: 2,
        new_price: 10500,
        new_qty: 200,
    }));
    
    // Should still be an ask
    assert_eq!(engine.best_ask(), Some(10500));
    assert_eq!(engine.best_bid(), None);
}

#[test]
fn test_modify_nonexistent() {
    let mut engine = Engine::new(1000);
    
    let events = engine.process_command(Command::Modify(flash_lob::ModifyOrder {
        order_id: 999,
        new_order_id: 1000,
        new_price: 10000,
        new_qty: 100,
    }));
    
    assert!(events.iter().any(|e| matches!(e, OutputEvent::Rejected(_))));
}

// ============================================================================
// Matching Edge Cases
// ============================================================================

#[test]
fn test_self_trade_allowed() {
    let mut engine = Engine::new(1000);
    
    // Same user on both sides (self-trade)
    engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 1,
        user_id: 100,
        side: Side::Ask,
        price: 10000,
        qty: 100,
    }));
    
    let events = engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 2,
        user_id: 100, // Same user
        side: Side::Bid,
        price: 10000,
        qty: 100,
    }));
    
    // Self-trade should be allowed (no prevention)
    assert!(events.iter().any(|e| matches!(e, OutputEvent::Trade(_))));
}

#[test]
fn test_partial_match_across_levels() {
    let mut engine = Engine::new(1000);
    
    // Multiple ask levels with partial quantities
    engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 1, user_id: 1, side: Side::Ask, price: 10000, qty: 30,
    }));
    engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 2, user_id: 1, side: Side::Ask, price: 10010, qty: 50,
    }));
    engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 3, user_id: 1, side: Side::Ask, price: 10020, qty: 70,
    }));
    
    // Match 100 qty (should consume 30 + 50 + 20)
    let events = engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
        order_id: 4,
        user_id: 2,
        side: Side::Bid,
        price: 10020,
        qty: 100,
    }));
    
    let trades: Vec<_> = events.iter()
        .filter_map(|e| if let OutputEvent::Trade(t) = e { Some((t.price, t.qty)) } else { None })
        .collect();
    
    assert_eq!(trades, vec![(10000, 30), (10010, 50), (10020, 20)]);
    
    // Order 3 should have 50 remaining
    assert_eq!(engine.order_count(), 1);
}

// ============================================================================
// Large Scale Fuzzing
// ============================================================================

#[test]
fn test_large_random_workload() {
    const SEED: u64 = 0xABCDEF123456;
    const OPS: usize = 50_000;
    
    let mut rng = ChaCha8Rng::seed_from_u64(SEED);
    let mut engine = Engine::new(100_000);
    
    let mut next_order_id = 1u64;
    let mut resting_orders = Vec::new();
    let mut total_trades = 0u64;
    let mut total_cancels = 0u64;
    
    for _ in 0..OPS {
        let op = rng.gen_range(0..100);
        
        if op < 60 {
            // 60% place
            let events = engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
                order_id: next_order_id,
                user_id: rng.gen_range(1..1000),
                side: if rng.gen_bool(0.5) { Side::Bid } else { Side::Ask },
                price: rng.gen_range(9000..11000) * 100,
                qty: rng.gen_range(1..500),
            }));
            
            if events.iter().any(|e| matches!(e, OutputEvent::Accepted(_))) {
                resting_orders.push(next_order_id);
            }
            
            total_trades += events.iter()
                .filter(|e| matches!(e, OutputEvent::Trade(_)))
                .count() as u64;
            
            next_order_id += 1;
        } else if op < 90 && !resting_orders.is_empty() {
            // 30% cancel
            let idx = rng.gen_range(0..resting_orders.len());
            let order_id = resting_orders.swap_remove(idx);
            
            let events = engine.process_command(Command::Cancel(CancelOrder { order_id }));
            
            if events.iter().any(|e| matches!(e, OutputEvent::Canceled(_))) {
                total_cancels += 1;
            }
        } else if !resting_orders.is_empty() {
            // 10% modify
            let idx = rng.gen_range(0..resting_orders.len());
            let order_id = resting_orders.swap_remove(idx);
            
            let events = engine.process_command(Command::Modify(flash_lob::ModifyOrder {
                order_id,
                new_order_id: next_order_id,
                new_price: rng.gen_range(9000..11000) * 100,
                new_qty: rng.gen_range(1..500),
            }));
            
            if events.iter().any(|e| matches!(e, OutputEvent::Accepted(_))) {
                resting_orders.push(next_order_id);
            }
            
            next_order_id += 1;
        }
    }
    
    println!("Large workload test completed:");
    println!("  Operations: {}", OPS);
    println!("  Orders placed: {}", next_order_id - 1);
    println!("  Total trades: {}", total_trades);
    println!("  Total cancels: {}", total_cancels);
    println!("  Final book size: {}", engine.order_count());
}

// ============================================================================
// Memory Leak Detection
// ============================================================================

#[test]
fn test_arena_returns_all_slots() {
    const CAPACITY: u32 = 1000;
    let mut engine = Engine::new(CAPACITY);
    
    // Add all orders with non-overlapping prices: bids 5000-5999, asks 15000-15999
    for i in 0..CAPACITY as u64 {
        let (side, price) = if i % 2 == 0 {
            (Side::Bid, 5000 + (i / 2) % 500)
        } else {
            (Side::Ask, 15000 + (i / 2) % 500)
        };
        engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
            order_id: i,
            user_id: 1,
            side,
            price,
            qty: 100,
        }));
    }
    
    assert_eq!(engine.order_count(), CAPACITY as usize);
    
    // Cancel all orders
    for i in 0..CAPACITY as u64 {
        engine.process_command(Command::Cancel(CancelOrder { order_id: i }));
    }
    
    assert_eq!(engine.order_count(), 0);
    
    // Should be able to fill again (arena slots reused)
    for i in 0..CAPACITY as u64 {
        let events = engine.process_command(Command::Place(PlaceOrder { order_type: flash_lob::OrderType::Limit,
            order_id: i + CAPACITY as u64,
            user_id: 1,
            side: Side::Bid,
            price: 10000,
            qty: 100,
        }));
        
        assert!(
            events.iter().any(|e| matches!(e, OutputEvent::Accepted(_))),
            "Order {} should be accepted after arena reset", i
        );
    }
}

// ============================================================================
// IOC/FOK Order Type Tests
// ============================================================================

#[test]
fn test_ioc_stress() {
    let mut engine = Engine::new(10_000);
    
    // Pre-populate with small liquidity across multiple price levels
    for i in 0..100 {
        engine.process_command(Command::Place(PlaceOrder {
            order_id: i,
            user_id: 1,
            side: Side::Ask,
            price: 10000 + (i % 20), // Spread across 20 price levels
            qty: 10,
            order_type: OrderType::Limit,
        }));
    }
    
    let initial_count = engine.order_count();
    
    // Send many IOC orders that don't cross (should all silently fail)
    for i in 100..200 {
        let events = engine.process_command(Command::Place(PlaceOrder {
            order_id: i,
            user_id: 2,
            side: Side::Bid,
            price: 9000, // Below all asks, won't match
            qty: 100,
            order_type: OrderType::IOC,
        }));
        
        // IOC that doesn't match should have zero events (no trades, no accepted)
        let accepted = events.iter().filter(|e| matches!(e, OutputEvent::Accepted(_))).count();
        let trades = events.iter().filter(|e| matches!(e, OutputEvent::Trade(_))).count();
        assert_eq!(accepted, 0, "IOC order should never rest in book");
        assert_eq!(trades, 0, "Non-crossing IOC should have no trades");
    }
    
    // Book should be unchanged
    assert_eq!(engine.order_count(), initial_count, "Non-crossing IOC should not affect book");
}

#[test]
fn test_fok_stress() {
    let mut engine = Engine::new(10_000);
    
    // Pre-populate with consistent liquidity
    for i in 0..100 {
        engine.process_command(Command::Place(PlaceOrder {
            order_id: i,
            user_id: 1,
            side: Side::Ask,
            price: 10000,
            qty: 100,
            order_type: OrderType::Limit,
        }));
    }
    
    // Total available: 10,000
    let mut filled = 0;
    let mut rejected = 0;
    
    // Try many FOK orders with varying sizes
    for i in 100..200 {
        let qty = (i - 100) * 50 + 10; // 10, 60, 110, 160, ...
        let events = engine.process_command(Command::Place(PlaceOrder {
            order_id: i,
            user_id: 2,
            side: Side::Bid,
            price: 10000,
            qty: qty as u32,
            order_type: OrderType::FOK,
        }));
        
        if events.iter().any(|e| matches!(e, OutputEvent::Trade(_))) {
            filled += 1;
        }
        if events.iter().any(|e| matches!(e, OutputEvent::Rejected(_))) {
            rejected += 1;
        }
    }
    
    // Some should fill, some should reject
    assert!(filled > 0, "Some FOK orders should fill");
    assert!(rejected > 0, "Some FOK orders should reject due to insufficient liquidity");
    println!("FOK stress: {} filled, {} rejected", filled, rejected);
}

#[test]
fn test_ioc_large_sweep() {
    let mut engine = Engine::new(10_000);
    
    // Pre-populate 1000 small orders across 10 price levels
    for i in 0..1000 {
        engine.process_command(Command::Place(PlaceOrder {
            order_id: i,
            user_id: 1,
            side: Side::Ask,
            price: 10000 + (i % 10),
            qty: 10,
            order_type: OrderType::Limit,
        }));
    }
    
    // Large IOC sweep
    let events = engine.process_command(Command::Place(PlaceOrder {
        order_id: 10000,
        user_id: 2,
        side: Side::Bid,
        price: 10009,
        qty: 50000, // More than available
        order_type: OrderType::IOC,
    }));
    
    // Should have many trades (sweeping through multiple levels)
    let trades = events.iter().filter(|e| matches!(e, OutputEvent::Trade(_))).count();
    assert!(trades > 100, "Large IOC should generate many trades, got {}", trades);
    
    // No resting order
    let accepted = events.iter().filter(|e| matches!(e, OutputEvent::Accepted(_))).count();
    assert_eq!(accepted, 0, "IOC should not rest");
}

