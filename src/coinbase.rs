use serde::Deserialize;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use chrono::{DateTime, Utc};
use crate::command::Side;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[derive(Debug, Deserialize)]
pub struct TardisL3Row {
    pub r#type: String,
    pub side: Option<String>,
    pub price: Option<Decimal>,
    pub amount: Option<Decimal>,
    pub order_id: Option<String>, // Changed to String to handle UUIDs
    pub trade_id: Option<u64>,
    pub timestamp: Option<DateTime<Utc>>,
    pub local_timestamp: Option<u64>,
}

#[derive(Debug)]
pub enum CoinbaseMessage {
    Received {
        order_id: u64,
        side: Side,
        price: u64, // Converted to cents/satoshis
        qty: u32,
    },
    Open {
        order_id: u64,
        side: Side,
        price: u64,
        qty: u32,
    },
    Done {
        order_id: u64,
        side: Side,
        reason: DoneReason,
    },
    Match {
        maker_order_id: u64,
        taker_order_id: u64,
        price: u64,
        qty: u32,
    },
    Change {
        order_id: u64,
        new_qty: u32,
        price: u64, // Price usually doesn't change, but included
    },
}

#[derive(Debug, PartialEq)]
pub enum DoneReason {
    Filled,
    Canceled,
}

impl TardisL3Row {
    /// Convert raw row to typed internal message
    /// Price multiplier: e.g. 100 for cents, 100000000 for satoshis
    pub fn to_message(&self, price_mult: u64) -> Option<CoinbaseMessage> {
        let side = match self.side.as_deref() {
            Some("buy") | Some("bid") => Side::Bid,
            Some("sell") | Some("ask") => Side::Ask,
            _ => Side::Bid, // Default, mostly relevant for types that have side
        };
        
        let price = self.price.map(|d| (d * Decimal::from(price_mult)).to_u64().unwrap_or(0));
        let qty = self.amount.map(|d| (d * Decimal::from(100000000u64)).to_u32().unwrap_or(0)); // Assuming max 8 decimals for size
        
        // Hash the UUID string to a u64
        let raw_id = self.order_id.as_deref().unwrap_or("0");
        let mut hasher = DefaultHasher::new();
        raw_id.hash(&mut hasher);
        let order_id = hasher.finish();
        
        match self.r#type.as_str() {
            "received" => Some(CoinbaseMessage::Received {
                order_id,
                side,
                price: price.unwrap_or(0),
                qty: qty.unwrap_or(0),
            }),
            "open" => Some(CoinbaseMessage::Open {
                order_id,
                side,
                price: price.unwrap_or(0),
                qty: qty.unwrap_or(0),
            }),
            "done" => {
                // Done messages can be filled or canceled
                // We infer reason? Tardis usually has 'reason' column but we didn't add it to struct
                // For simplified replay, 'done' implies remove from book.
                Some(CoinbaseMessage::Done {
                    order_id,
                    side,
                    reason: DoneReason::Canceled, // Simplification for now
                })
            },
            "match" => Some(CoinbaseMessage::Match {
                maker_order_id: order_id, // For match, order_id is usually maker
                taker_order_id: self.trade_id.unwrap_or(0), // Taker/Trade ID? Validation requires care
                price: price.unwrap_or(0),
                qty: qty.unwrap_or(0),
            }),
            "change" => Some(CoinbaseMessage::Change {
                order_id,
                new_qty: qty.unwrap_or(0),
                price: price.unwrap_or(0),
            }),
            _ => None,
        }
    }
}
