//! # Flash-LOB
//!
//! A deterministic, high-frequency limit order book matching engine.
//!
//! ## Design Principles
//!
//! - **Single-Writer**: One thread owns the order book exclusively (no locks)
//! - **O(1) Operations**: Insert, Cancel, Match all run in constant time
//! - **Cache-Optimized**: 64-byte aligned nodes, 32-bit indices
//! - **Arena Allocation**: No heap allocation in the hot path
//!
//! ## Architecture
//!
//! ```text
//! [Network Thread] --> [SPSC Ring Buffer] --> [Engine Thread (Pinned)]
//!                                                     |
//!                                              [Output Events]
//! ```

pub mod arena;
pub mod command;
pub mod price_level;
pub mod order_book;
pub mod matching;
pub mod engine;

// Re-exports for convenience
pub use arena::{Arena, ArenaIndex, OrderNode, NULL_INDEX};
pub use command::{Command, PlaceOrder, CancelOrder, Side, TradeEvent, BookUpdate, OutputEvent};
pub use price_level::PriceLevel;
pub use order_book::OrderBook;
pub use engine::Engine;
