//! Fuzz Test - Compares Flash-LOB against a reference implementation.
//!
//! Uses a naive but correct reference implementation to verify
//! the optimized engine produces identical results.

use flash_lob::{Engine, Command, PlaceOrder, CancelOrder, Side, OutputEvent};
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use std::collections::BTreeMap;

/// Simple reference implementation for verification
struct ReferenceBook {
    bids: BTreeMap<u64, Vec<(u64, u32)>>, // price -> [(order_id, qty)]
    asks: BTreeMap<u64, Vec<(u64, u32)>>,
    orders: std::collections::HashMap<u64, (Side, u64)>, // order_id -> (side, price)
}

impl ReferenceBook {
    fn new() -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            orders: std::collections::HashMap::new(),
        }
    }
    
    fn best_bid(&self) -> Option<u64> {
        self.bids.iter().rev().find(|(_, v)| !v.is_empty()).map(|(k, _)| *k)
    }
    
    fn best_ask(&self) -> Option<u64> {
        self.asks.iter().find(|(_, v)| !v.is_empty()).map(|(k, _)| *k)
    }
    
    fn place(&mut self, order_id: u64, side: Side, price: u64, mut qty: u32) -> u32 {
        // Simple crossing (no partial fills tracking, just quantity consumed)
        let mut traded = 0u32;
        
        match side {
            Side::Bid => {
                // Match against asks
                let mut prices_to_remove = Vec::new();
                for (&ask_price, orders) in self.asks.iter_mut() {
                    if ask_price > price || qty == 0 {
                        break;
                    }
                    while !orders.is_empty() && qty > 0 {
                        let trade_qty = orders[0].1.min(qty);
                        orders[0].1 -= trade_qty;
                        qty -= trade_qty;
                        traded += trade_qty;
                        
                        if orders[0].1 == 0 {
                            let (maker_id, _) = orders.remove(0);
                            self.orders.remove(&maker_id);
                        }
                    }
                    if orders.is_empty() {
                        prices_to_remove.push(ask_price);
                    }
                }
                for p in prices_to_remove {
                    self.asks.remove(&p);
                }
                
                // Rest
                if qty > 0 {
                    self.bids.entry(price).or_default().push((order_id, qty));
                    self.orders.insert(order_id, (Side::Bid, price));
                }
            }
            Side::Ask => {
                // Match against bids (highest first)
                let mut prices_to_remove = Vec::new();
                let prices: Vec<_> = self.bids.keys().rev().copied().collect();
                for bid_price in prices {
                    if bid_price < price || qty == 0 {
                        break;
                    }
                    let orders = self.bids.get_mut(&bid_price).unwrap();
                    while !orders.is_empty() && qty > 0 {
                        let trade_qty = orders[0].1.min(qty);
                        orders[0].1 -= trade_qty;
                        qty -= trade_qty;
                        traded += trade_qty;
                        
                        if orders[0].1 == 0 {
                            let (maker_id, _) = orders.remove(0);
                            self.orders.remove(&maker_id);
                        }
                    }
                    if orders.is_empty() {
                        prices_to_remove.push(bid_price);
                    }
                }
                for p in prices_to_remove {
                    self.bids.remove(&p);
                }
                
                // Rest
                if qty > 0 {
                    self.asks.entry(price).or_default().push((order_id, qty));
                    self.orders.insert(order_id, (Side::Ask, price));
                }
            }
        }
        
        traded
    }
    
    fn cancel(&mut self, order_id: u64) -> bool {
        if let Some((side, price)) = self.orders.remove(&order_id) {
            let book = match side {
                Side::Bid => &mut self.bids,
                Side::Ask => &mut self.asks,
            };
            if let Some(orders) = book.get_mut(&price) {
                orders.retain(|(id, _)| *id != order_id);
                if orders.is_empty() {
                    book.remove(&price);
                }
            }
            true
        } else {
            false
        }
    }
    
    fn order_count(&self) -> usize {
        self.orders.len()
    }
}

fn generate_command(rng: &mut ChaCha8Rng, order_id: u64) -> PlaceOrder {
    PlaceOrder {
        order_id,
        user_id: rng.gen_range(1..100),
        side: if rng.gen_bool(0.5) { Side::Bid } else { Side::Ask },
        price: rng.gen_range(9800..10200) * 100,
        qty: rng.gen_range(1..200),
        order_type: flash_lob::OrderType::Limit,
    }
}

#[test]
fn test_fuzz_best_prices() {
    const SEED: u64 = 0xFEEDFACE;
    const OPS: usize = 10_000;
    
    let mut rng = ChaCha8Rng::seed_from_u64(SEED);
    let mut engine = Engine::new(100_000);
    let mut reference = ReferenceBook::new();
    
    let mut next_order_id = 1u64;
    let mut active_orders: Vec<u64> = Vec::new();
    
    for i in 0..OPS {
        // 70% place, 30% cancel
        if active_orders.is_empty() || rng.gen_bool(0.7) {
            let order = generate_command(&mut rng, next_order_id);
            next_order_id += 1;
            
            // Run both
            engine.process_command(Command::Place(order));
            reference.place(order.order_id, order.side, order.price, order.qty);
            
            // Track if it might be resting
            active_orders.push(order.order_id);
        } else {
            let idx = rng.gen_range(0..active_orders.len());
            let order_id = active_orders.swap_remove(idx);
            
            engine.process_command(Command::Cancel(CancelOrder { order_id }));
            reference.cancel(order_id);
        }
        
        // Compare best prices
        let engine_bid = engine.best_bid();
        let engine_ask = engine.best_ask();
        let ref_bid = reference.best_bid();
        let ref_ask = reference.best_ask();
        
        assert_eq!(
            engine_bid, ref_bid,
            "Best bid mismatch at op {}: engine={:?}, reference={:?}",
            i, engine_bid, ref_bid
        );
        assert_eq!(
            engine_ask, ref_ask,
            "Best ask mismatch at op {}: engine={:?}, reference={:?}",
            i, engine_ask, ref_ask
        );
    }
    
    println!("Fuzz test passed!");
    println!("  Operations: {}", OPS);
    println!("  Final order count - Engine: {}, Reference: {}", 
             engine.order_count(), reference.order_count());
}

#[test]
fn test_fuzz_order_count() {
    const SEED: u64 = 0xBADC0DE;
    const OPS: usize = 5_000;
    
    let mut rng = ChaCha8Rng::seed_from_u64(SEED);
    let mut engine = Engine::new(100_000);
    let mut reference = ReferenceBook::new();
    
    let mut next_order_id = 1u64;
    let mut active_orders: Vec<u64> = Vec::new();
    
    for i in 0..OPS {
        if active_orders.is_empty() || rng.gen_bool(0.6) {
            let order = generate_command(&mut rng, next_order_id);
            next_order_id += 1;
            
            let events = engine.process_command(Command::Place(order));
            reference.place(order.order_id, order.side, order.price, order.qty);
            
            // Check if order is resting
            let is_resting = events.iter().any(|e| matches!(e, OutputEvent::Accepted(_)));
            if is_resting {
                active_orders.push(order.order_id);
            }
        } else {
            let idx = rng.gen_range(0..active_orders.len());
            let order_id = active_orders.swap_remove(idx);
            
            engine.process_command(Command::Cancel(CancelOrder { order_id }));
            reference.cancel(order_id);
        }
        
        // Compare order counts periodically
        if i % 100 == 0 {
            assert_eq!(
                engine.order_count(), reference.order_count(),
                "Order count mismatch at op {}", i
            );
        }
    }
    
    // Final comparison
    assert_eq!(engine.order_count(), reference.order_count());
    println!("Order count fuzz test passed!");
}

#[test]
fn test_fuzz_trade_volume() {
    const SEED: u64 = 0x12345678;
    const OPS: usize = 5_000;
    
    let mut rng = ChaCha8Rng::seed_from_u64(SEED);
    let mut engine = Engine::new(100_000);
    let mut reference = ReferenceBook::new();
    
    let mut engine_traded = 0u64;
    let mut reference_traded = 0u64;
    
    for i in 0..OPS {
        let order = generate_command(&mut rng, i as u64);
        
        let events = engine.process_command(Command::Place(order));
        let ref_qty = reference.place(order.order_id, order.side, order.price, order.qty);
        
        // Sum traded volume from engine events
        let engine_qty: u32 = events.iter()
            .filter_map(|e| if let OutputEvent::Trade(t) = e { Some(t.qty) } else { None })
            .sum();
        
        engine_traded += engine_qty as u64;
        reference_traded += ref_qty as u64;
    }
    
    assert_eq!(
        engine_traded, reference_traded,
        "Total traded volume mismatch: engine={}, reference={}",
        engine_traded, reference_traded
    );
    
    println!("Trade volume fuzz test passed!");
    println!("  Total traded: {}", engine_traded);
}
