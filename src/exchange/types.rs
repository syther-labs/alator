use std::{cmp::Ordering, sync::Arc};

use crate::types::DateTime;

pub type PriceSender<Q> = tokio::sync::mpsc::Sender<Vec<Arc<Q>>>;
pub type PriceReceiver<Q> = tokio::sync::mpsc::Receiver<Vec<Arc<Q>>>;
pub type NotifySender = tokio::sync::mpsc::Sender<ExchangeNotificationMessage>;
pub type NotifyReceiver = tokio::sync::mpsc::Receiver<ExchangeNotificationMessage>;
pub type OrderSender = tokio::sync::mpsc::Sender<ExchangeOrderMessage>;
pub type OrderReciever = tokio::sync::mpsc::Receiver<ExchangeOrderMessage>;

pub type DefaultExchangeOrderId = u32;
pub type DefaultSubscriberId = u8;

//Supported order types for the exchange
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderType {
    MarketSell,
    MarketBuy,
    LimitSell,
    LimitBuy,
    StopSell,
    StopBuy,
}

#[derive(Clone, Debug)]
pub struct ExchangeOrder {
    pub subscriber_id: DefaultSubscriberId,
    pub order_type: OrderType,
    pub symbol: String,
    pub shares: f64,
    pub price: Option<f64>,
}

impl ExchangeOrder {
    pub fn get_subscriber_id(&self) -> &DefaultSubscriberId {
        &self.subscriber_id
    }

    pub fn get_symbol(&self) -> &String {
        &self.symbol
    }

    pub fn get_shares(&self) -> &f64 {
        &self.shares
    }

    pub fn get_price(&self) -> &Option<f64> {
        &self.price
    }

    pub fn get_order_type(&self) -> &OrderType {
        &self.order_type
    }

    pub fn market(
        subscriber_id: DefaultSubscriberId,
        order_type: OrderType,
        symbol: impl Into<String>,
        shares: f64,
    ) -> Self {
        Self {
            subscriber_id,
            order_type,
            symbol: symbol.into(),
            shares,
            price: None,
        }
    }

    pub fn market_buy(
        subscriber_id: DefaultSubscriberId,
        symbol: impl Into<String>,
        shares: f64,
    ) -> Self {
        ExchangeOrder::market(subscriber_id, OrderType::MarketBuy, symbol, shares)
    }

    pub fn market_sell(
        subscriber_id: DefaultSubscriberId,
        symbol: impl Into<String>,
        shares: f64,
    ) -> Self {
        ExchangeOrder::market(subscriber_id, OrderType::MarketSell, symbol, shares)
    }

    pub fn delayed(
        subscriber_id: DefaultSubscriberId,
        order_type: OrderType,
        symbol: impl Into<String>,
        shares: f64,
        price: f64,
    ) -> Self {
        Self {
            subscriber_id,
            order_type,
            symbol: symbol.into(),
            shares,
            price: Some(price),
        }
    }

    pub fn stop_buy(
        subscriber_id: DefaultSubscriberId,
        symbol: impl Into<String>,
        shares: f64,
        price: f64,
    ) -> Self {
        ExchangeOrder::delayed(subscriber_id, OrderType::StopBuy, symbol, shares, price)
    }

    pub fn stop_sell(
        subscriber_id: DefaultSubscriberId,
        symbol: impl Into<String>,
        shares: f64,
        price: f64,
    ) -> Self {
        ExchangeOrder::delayed(subscriber_id, OrderType::StopSell, symbol, shares, price)
    }

    pub fn limit_buy(
        subscriber_id: DefaultSubscriberId,
        symbol: impl Into<String>,
        shares: f64,
        price: f64,
    ) -> Self {
        ExchangeOrder::delayed(subscriber_id, OrderType::LimitBuy, symbol, shares, price)
    }

    pub fn limit_sell(
        subscriber_id: DefaultSubscriberId,
        symbol: impl Into<String>,
        shares: f64,
        price: f64,
    ) -> Self {
        ExchangeOrder::delayed(subscriber_id, OrderType::LimitSell, symbol, shares, price)
    }
}

impl Eq for ExchangeOrder {}

impl PartialEq for ExchangeOrder {
    fn eq(&self, other: &Self) -> bool {
        self.symbol == other.symbol
            && self.order_type == other.order_type
            && self.shares == other.shares
    }
}

#[derive(Clone, Copy, Debug)]
pub enum TradeType {
    Buy,
    Sell,
}

#[derive(Clone, Debug)]
pub struct ExchangeTrade {
    pub subscriber_id: DefaultSubscriberId,
    pub symbol: String,
    pub value: f64,
    pub quantity: f64,
    pub date: DateTime,
    pub typ: TradeType,
}

impl ExchangeTrade {
    pub fn new(
        subscriber_id: DefaultSubscriberId,
        symbol: impl Into<String>,
        value: f64,
        quantity: f64,
        date: impl Into<DateTime>,
        typ: TradeType,
    ) -> Self {
        Self {
            subscriber_id,
            symbol: symbol.into(),
            value,
            quantity,
            date: date.into(),
            typ,
        }
    }
}

impl Ord for ExchangeTrade {
    fn cmp(&self, other: &Self) -> Ordering {
        self.date.cmp(&other.date)
    }
}

impl PartialOrd for ExchangeTrade {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for ExchangeTrade {}

impl PartialEq for ExchangeTrade {
    fn eq(&self, other: &Self) -> bool {
        self.date == other.date && self.symbol == other.symbol
    }
}

pub enum ExchangeNotificationMessage {
    TradeCompleted(ExchangeTrade),
    OrderBooked(DefaultExchangeOrderId, ExchangeOrder),
    OrderDeleted(DefaultExchangeOrderId),
}

pub enum ExchangeOrderMessage {
    CreateOrder(ExchangeOrder),
    DeleteOrder(DefaultSubscriberId, DefaultExchangeOrderId),
    ClearOrdersBySymbol(DefaultSubscriberId, String),
}

impl ExchangeOrderMessage {
    pub fn market_buy(
        subscriber_id: DefaultSubscriberId,
        symbol: impl Into<String>,
        shares: f64,
    ) -> Self {
        ExchangeOrderMessage::CreateOrder(ExchangeOrder::market_buy(subscriber_id, symbol, shares))
    }

    pub fn market_sell(
        subscriber_id: DefaultSubscriberId,
        symbol: impl Into<String>,
        shares: f64,
    ) -> Self {
        ExchangeOrderMessage::CreateOrder(ExchangeOrder::market_sell(subscriber_id, symbol, shares))
    }

    pub fn stop_buy(
        subscriber_id: DefaultSubscriberId,
        symbol: impl Into<String>,
        shares: f64,
        price: f64,
    ) -> Self {
        ExchangeOrderMessage::CreateOrder(ExchangeOrder::stop_buy(
            subscriber_id,
            symbol,
            shares,
            price,
        ))
    }

    pub fn stop_sell(
        subscriber_id: DefaultSubscriberId,
        symbol: impl Into<String>,
        shares: f64,
        price: f64,
    ) -> Self {
        ExchangeOrderMessage::CreateOrder(ExchangeOrder::stop_sell(
            subscriber_id,
            symbol,
            shares,
            price,
        ))
    }

    pub fn limit_buy(
        subscriber_id: DefaultSubscriberId,
        symbol: impl Into<String>,
        shares: f64,
        price: f64,
    ) -> Self {
        ExchangeOrderMessage::CreateOrder(ExchangeOrder::limit_buy(
            subscriber_id,
            symbol,
            shares,
            price,
        ))
    }

    pub fn limit_sell(
        subscriber_id: DefaultSubscriberId,
        symbol: impl Into<String>,
        shares: f64,
        price: f64,
    ) -> Self {
        ExchangeOrderMessage::CreateOrder(ExchangeOrder::limit_sell(
            subscriber_id,
            symbol,
            shares,
            price,
        ))
    }
}