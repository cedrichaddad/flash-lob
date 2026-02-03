//! Engine - Main event loop with CPU pinning and warm-up.
//!
//! Wraps the matching engine with I/O handling via rtrb ring buffers.

use crate::command::{Command, OutputEvent};
use crate::matching::MatchingEngine;

/// The main engine that processes commands from a ring buffer.
///
/// Uses the rtrb crate for lock-free SPSC communication.
pub struct Engine {
    /// The underlying matching engine
    pub matcher: MatchingEngine,
}

impl Engine {
    /// Create a new engine with the specified order capacity.
    pub fn new(capacity: u32) -> Self {
        Self {
            matcher: MatchingEngine::new(capacity),
        }
    }
    
    /// Run the engine event loop.
    ///
    /// # Arguments
    /// * `input` - Consumer end of the command ring buffer
    /// * `output` - Producer end of the output event ring buffer
    /// * `pin_to_core` - Whether to pin to the last available CPU core
    ///
    /// # Note
    /// This function runs forever (until the program terminates).
    #[cfg(feature = "runtime")]
    pub fn run(
        &mut self,
        input: &mut rtrb::Consumer<Command>,
        output: &mut rtrb::Producer<OutputEvent>,
        pin_to_core: bool,
    ) {
        // Pin to isolated CPU core
        if pin_to_core {
            self.pin_to_core();
        }
        
        // Warm-up phase
        self.warm_up();
        
        // Main event loop (busy-wait)
        loop {
            while let Ok(cmd) = input.pop() {
                let events = self.process_command(cmd);
                for event in events {
                    // Best effort - drop if full
                    let _ = output.push(event);
                }
            }
            std::hint::spin_loop();
        }
    }
    
    /// Process a single command and return output events.
    ///
    /// This is the main entry point for synchronous usage (testing, benchmarks).
    #[inline]
    pub fn process_command(&mut self, cmd: Command) -> Vec<OutputEvent> {
        match cmd {
            Command::Place(order) => self.matcher.process_place(order),
            Command::Cancel(cancel) => self.matcher.process_cancel(cancel),
            Command::Modify(modify) => {
                // First retrieve the original order info before canceling
                let original_info = self.matcher.book.get_order(modify.order_id).copied();
                
                // Modify = Cancel + Place
                let mut events = self.matcher.process_cancel(crate::command::CancelOrder {
                    order_id: modify.order_id,
                });
                
                // Only place if cancel succeeded and we had the original order info
                let cancel_succeeded = events.iter().any(|e| {
                    matches!(e, OutputEvent::Canceled(_))
                });
                
                if cancel_succeeded {
                    if let Some(info) = original_info {
                        let place_events = self.matcher.process_place(crate::command::PlaceOrder {
                            order_id: modify.new_order_id,
                            user_id: info.user_id,
                            side: info.side,
                            price: modify.new_price,
                            qty: modify.new_qty,
                            order_type: crate::command::OrderType::Limit,
                        });
                        events.extend(place_events);
                    }
                }
                
                events
            }
        }
    }
    
    /// Pin the current thread to the last available CPU core.
    ///
    /// The last core is typically isolated from OS interrupts.
    pub fn pin_to_core(&self) {
        if let Some(core_ids) = core_affinity::get_core_ids() {
            if let Some(last_core) = core_ids.last() {
                core_affinity::set_for_current(*last_core);
            }
        }
    }
    
    /// Warm up the engine by pre-faulting memory pages.
    pub fn warm_up(&mut self) {
        self.matcher.warm_up();
    }
    
    /// Get the best bid price.
    #[inline]
    pub fn best_bid(&self) -> Option<u64> {
        self.matcher.best_bid()
    }
    
    /// Get the best ask price.
    #[inline]
    pub fn best_ask(&self) -> Option<u64> {
        self.matcher.best_ask()
    }
    
    /// Get the spread.
    #[inline]
    pub fn spread(&self) -> Option<u64> {
        self.matcher.spread()
    }
    
    /// Get total order count.
    #[inline]
    pub fn order_count(&self) -> usize {
        self.matcher.order_count()
    }
    
    /// Compute state hash for determinism testing.
    #[inline]
    pub fn state_hash(&self) -> u64 {
        self.matcher.state_hash()
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new(1_000_000) // 1M orders default capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{PlaceOrder, CancelOrder, Side, OrderType};
    
    #[test]
    fn test_engine_creation() {
        let engine = Engine::new(10000);
        assert_eq!(engine.order_count(), 0);
        assert_eq!(engine.best_bid(), None);
        assert_eq!(engine.best_ask(), None);
    }
    
    #[test]
    fn test_engine_process_place() {
        let mut engine = Engine::new(1000);
        
        let cmd = Command::Place(PlaceOrder {
            order_id: 1,
            user_id: 100,
            side: Side::Bid,
            price: 10000,
            qty: 100,
            order_type: OrderType::Limit,
        });
        
        let events = engine.process_command(cmd);
        assert!(!events.is_empty());
        assert_eq!(engine.order_count(), 1);
        assert_eq!(engine.best_bid(), Some(10000));
    }
    
    #[test]
    fn test_engine_process_cancel() {
        let mut engine = Engine::new(1000);
        
        // Place
        engine.process_command(Command::Place(PlaceOrder {
            order_id: 1,
            user_id: 100,
            side: Side::Bid,
            price: 10000,
            qty: 100,
            order_type: OrderType::Limit,
        }));
        
        // Cancel
        let events = engine.process_command(Command::Cancel(CancelOrder {
            order_id: 1,
        }));
        
        assert!(!events.is_empty());
        assert_eq!(engine.order_count(), 0);
    }
    
    #[test]
    fn test_engine_state_hash_determinism() {
        let mut engine1 = Engine::new(1000);
        let mut engine2 = Engine::new(1000);
        
        // Same operations
        for i in 0..100 {
            let cmd = Command::Place(PlaceOrder {
                order_id: i,
                user_id: 1,
                side: if i % 2 == 0 { Side::Bid } else { Side::Ask },
                price: 10000 + (i % 10) * 10,
                qty: 100,
                order_type: OrderType::Limit,
            });
            engine1.process_command(cmd);
            engine2.process_command(cmd);
        }
        
        assert_eq!(engine1.state_hash(), engine2.state_hash());
    }
    
    #[test]
    fn test_engine_warm_up() {
        let mut engine = Engine::new(1000);
        engine.warm_up(); // Should not panic
    }
}
