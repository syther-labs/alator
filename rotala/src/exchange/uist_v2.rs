use core::panic;
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Display,
    mem,
};

use serde::{Deserialize, Serialize};

use crate::source::hyperliquid::{DateDepth, DateTrade, Depth, BBO};

pub type OrderId = u64;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Quote {
    pub bid: f64,
    pub bid_volume: f64,
    pub ask: f64,
    pub ask_volume: f64,
    pub date: i64,
    pub symbol: String,
}

impl From<BBO> for Quote {
    fn from(value: BBO) -> Self {
        Self {
            bid: value.bid,
            bid_volume: value.bid_volume,
            ask: value.ask,
            ask_volume: value.ask_volume,
            date: value.date,
            symbol: value.symbol,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum OrderType {
    MarketSell,
    MarketBuy,
    LimitBuy,
    LimitSell,
    Cancel,
    Modify,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Order {
    pub order_type: OrderType,
    pub symbol: String,
    pub qty: f64,
    pub price: Option<f64>,
    pub order_id_ref: Option<OrderId>,
    pub exchange: String,
}

impl Order {
    fn market(
        order_type: OrderType,
        symbol: impl Into<String>,
        shares: f64,
        exchange: impl Into<String>,
    ) -> Self {
        Self {
            order_type,
            symbol: symbol.into(),
            qty: shares,
            price: None,
            order_id_ref: None,
            exchange: exchange.into(),
        }
    }

    fn delayed(
        order_type: OrderType,
        symbol: impl Into<String>,
        shares: f64,
        price: f64,
        exchange: impl Into<String>,
    ) -> Self {
        Self {
            order_type,
            symbol: symbol.into(),
            qty: shares,
            price: Some(price),
            order_id_ref: None,
            exchange: exchange.into(),
        }
    }

    pub fn market_buy(symbol: impl Into<String>, shares: f64, exchange: impl Into<String>) -> Self {
        Order::market(OrderType::MarketBuy, symbol, shares, exchange)
    }

    pub fn market_sell(
        symbol: impl Into<String>,
        shares: f64,
        exchange: impl Into<String>,
    ) -> Self {
        Order::market(OrderType::MarketSell, symbol, shares, exchange)
    }

    pub fn limit_buy(
        symbol: impl Into<String>,
        shares: f64,
        price: f64,
        exchange: impl Into<String>,
    ) -> Self {
        Order::delayed(OrderType::LimitBuy, symbol, shares, price, exchange)
    }

    pub fn limit_sell(
        symbol: impl Into<String>,
        shares: f64,
        price: f64,
        exchange: impl Into<String>,
    ) -> Self {
        Order::delayed(OrderType::LimitSell, symbol, shares, price, exchange)
    }

    pub fn modify_order(
        symbol: impl Into<String>,
        order_id: OrderId,
        qty_change: f64,
        exchange: impl Into<String>,
    ) -> Self {
        Self {
            order_id_ref: Some(order_id),
            order_type: OrderType::Modify,
            symbol: symbol.into(),
            price: None,
            qty: qty_change,
            exchange: exchange.into(),
        }
    }

    pub fn cancel_order(
        symbol: impl Into<String>,
        order_id: OrderId,
        exchange: impl Into<String>,
    ) -> Self {
        Self {
            order_id_ref: Some(order_id),
            order_type: OrderType::Cancel,
            symbol: symbol.into(),
            price: None,
            qty: 0.0,
            exchange: exchange.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum OrderResultType {
    Buy,
    Sell,
    Modify,
    Cancel,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OrderResult {
    pub symbol: String,
    pub value: f64,
    pub quantity: f64,
    pub date: i64,
    pub typ: OrderResultType,
    pub order_id: OrderId,
    pub order_id_ref: Option<OrderId>,
    pub exchange: String,
}

#[derive(Debug)]
pub struct UistV2 {
    orderbook: OrderBook,
    order_result_log: Vec<OrderResult>,
    //This is cleared on every tick
    order_buffer: Vec<Order>,
}

impl UistV2 {
    pub fn new() -> Self {
        Self {
            orderbook: OrderBook::default(),
            order_result_log: Vec::new(),
            order_buffer: Vec::new(),
        }
    }

    fn sort_order_buffer(&mut self) {
        self.order_buffer.sort_by(|a, _b| match a.order_type {
            OrderType::LimitSell | OrderType::MarketSell => std::cmp::Ordering::Less,
            _ => std::cmp::Ordering::Greater,
        })
    }

    pub fn insert_order(&mut self, order: Order) {
        // Orders are only inserted into the book when tick is called, this is to ensure proper
        // ordering of trades
        // This impacts order_id where an order X can come in before order X+1 but the latter can
        // have an order_id that is less than the former.
        self.order_buffer.push(order);
    }

    pub fn insert_orders(&mut self, mut orders: Vec<Order>) {
        let mut orders = mem::take(&mut orders);
        self.order_buffer.append(&mut orders);
    }

    pub fn tick(
        &mut self,
        quotes: &DateDepth,
        trades: &DateTrade,
        now: i64,
    ) -> (Vec<OrderResult>, Vec<InnerOrder>) {
        //To eliminate lookahead bias, we only insert new orders after we have executed any orders
        //that were on the stack first
        let executed_trades = self.orderbook.execute_orders(quotes, trades, now);
        for executed_trade in &executed_trades {
            self.order_result_log.push(executed_trade.clone());
        }
        let mut inserted_orders = Vec::new();

        self.sort_order_buffer();
        //TODO: remove this overhead, shouldn't need a clone here
        for order in self.order_buffer.iter() {
            let inner_order = self.orderbook.insert_order(order.clone(), now);
            inserted_orders.push(inner_order);
        }

        self.order_buffer.clear();
        (executed_trades, inserted_orders)
    }
}

impl Default for UistV2 {
    fn default() -> Self {
        Self::new()
    }
}

// FillTracker is stored over the life of an execution cycle.
// New data structure is created so that we do not have to modify the underlying quotes that are
// passed to the execute_orders function. Orderbook is intended to be relatively pure and so needs
// to hold the minimum amount of data itself. Modifying underlying quotes would mean copies, which
// would get expensive.
struct FillTracker {
    inner: HashMap<String, HashMap<String, f64>>,
}

impl FillTracker {
    fn get_fill(&self, symbol: &str, price: &f64) -> f64 {
        //Can default to zero instead of None
        if let Some(fills) = self.inner.get(symbol) {
            let level_string = price.to_string();
            if let Some(val) = fills.get(&level_string) {
                return *val;
            }
        }
        0.0
    }

    fn insert_fill(&mut self, symbol: &str, price: &f64, filled: f64) {
        if !self.inner.contains_key(symbol) {
            self.inner
                .insert(symbol.to_string().clone(), HashMap::new());
        }

        let fills = self.inner.get_mut(symbol).unwrap();
        let level_string = price.to_string();

        fills
            .entry(level_string)
            .and_modify(|count| *count += filled)
            .or_insert(filled);
    }

    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }
}

#[derive(Debug)]
pub enum LatencyModel {
    None,
    FixedPeriod(i64),
}

impl LatencyModel {
    fn cmp_order(&self, now: i64, order: &InnerOrder) -> bool {
        match self {
            Self::None => true,
            Self::FixedPeriod(period) => order.recieved_timestamp + period < now,
        }
    }
}

//Representation of order used internally, this is sent back to clients.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InnerOrder {
    pub order_type: OrderType,
    pub symbol: String,
    pub qty: f64,
    pub price: Option<f64>,
    pub recieved_timestamp: i64,
    pub order_id: OrderId,
    pub order_id_ref: Option<OrderId>,
    pub exchange: String,
}

#[derive(Debug)]
pub enum OrderBookOrderPriority {
    AlwaysFirst,
    TradeThrough,
}

#[derive(Debug)]
pub enum OrderBookError {
    OrderIdNotFound,
}

impl Display for OrderBookError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "OrderBookError")
    }
}

impl std::error::Error for OrderBookError {}

//The key is an f64 but we use a String because f64 does not impl Hash
type FilledTrades = HashMap<String, (f64, f64)>;

#[derive(Debug)]
pub struct OrderBook {
    inner: BTreeMap<OrderId, InnerOrder>,
    latency: LatencyModel,
    last_order_id: u64,
    priority_setting: OrderBookOrderPriority,
}

impl Default for OrderBook {
    fn default() -> Self {
        Self::new()
    }
}

impl OrderBook {
    pub fn new() -> Self {
        Self {
            inner: BTreeMap::new(),
            latency: LatencyModel::None,
            last_order_id: 0,
            priority_setting: OrderBookOrderPriority::AlwaysFirst,
        }
    }

    //Used for testing
    pub fn get_total_order_qty_by_symbol(&self, symbol: &str) -> f64 {
        let mut total = 0.0;
        for order in self.inner.values() {
            if order.symbol == symbol {
                total += order.qty
            }
        }
        total
    }

    pub fn with_latency(latency: i64) -> Self {
        Self {
            inner: BTreeMap::new(),
            latency: LatencyModel::FixedPeriod(latency),
            last_order_id: 0,
            priority_setting: OrderBookOrderPriority::AlwaysFirst,
        }
    }

    pub fn insert_order(&mut self, order: Order, now: i64) -> InnerOrder {
        let inner_order = InnerOrder {
            recieved_timestamp: now,
            order_id: self.last_order_id,
            order_type: order.order_type,
            symbol: order.symbol.clone(),
            qty: order.qty,
            price: order.price,
            order_id_ref: order.order_id_ref,
            exchange: order.exchange,
        };

        self.inner.insert(self.last_order_id, inner_order.clone());
        self.last_order_id += 1;
        inner_order
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    // Only returns a single `OrderResult` but we return a `Vec` for empty condition
    fn cancel_order(
        now: i64,
        cancel_order: &InnerOrder,
        orderbook: &mut BTreeMap<OrderId, InnerOrder>,
    ) -> Vec<OrderResult> {
        let mut res = Vec::new();
        //Fails silently if you send garbage in
        if let Some(order_to_cancel_id) = &cancel_order.order_id_ref {
            if orderbook.remove(order_to_cancel_id).is_some() {
                let order_result = OrderResult {
                    symbol: cancel_order.symbol.clone(),
                    value: 0.0,
                    quantity: 0.0,
                    date: now,
                    typ: OrderResultType::Cancel,
                    order_id: cancel_order.order_id,
                    order_id_ref: Some(*order_to_cancel_id),
                    exchange: cancel_order.exchange.clone(),
                };
                res.push(order_result);
            }
        }

        res
    }

    // Only returns a single `OrderResult` but we return a `Vec` for empty condition
    fn modify_order(
        now: i64,
        modify_order: &InnerOrder,
        orderbook: &mut BTreeMap<OrderId, InnerOrder>,
    ) -> Vec<OrderResult> {
        let mut res = Vec::new();

        if let Some(order_to_modify) = orderbook.get_mut(&modify_order.order_id_ref.unwrap()) {
            let qty_change = modify_order.qty;

            if qty_change > 0.0 {
                order_to_modify.qty += qty_change;
            } else {
                let qty_left = order_to_modify.qty + qty_change;
                if qty_left > 0.0 {
                    order_to_modify.qty += qty_change;
                } else {
                    // we are trying to remove more than the total number of shares
                    // left on the order so will assume user wants to cancel
                    orderbook.remove(&modify_order.order_id);
                }
            }

            let order_result = OrderResult {
                symbol: modify_order.symbol.clone(),
                value: 0.0,
                quantity: 0.0,
                date: now,
                typ: OrderResultType::Modify,
                order_id: modify_order.order_id,
                order_id_ref: Some(modify_order.order_id_ref.unwrap()),
                exchange: modify_order.exchange.clone(),
            };
            res.push(order_result);
        }
        res
    }

    fn fill_order(
        depth: &Depth,
        order: &InnerOrder,
        filled: &mut FillTracker,
        taker_trades: &FilledTrades,
        priority_setting: &OrderBookOrderPriority,
    ) -> Vec<OrderResult> {
        let mut to_fill = order.qty;
        let mut trades = Vec::new();

        let is_buy = match order.order_type {
            OrderType::MarketBuy | OrderType::LimitBuy => true,
            OrderType::LimitSell | OrderType::MarketSell => false,
            _ => panic!("Can't fill cancel or modify"),
        };

        let price_check = match order.order_type {
            OrderType::LimitBuy | OrderType::LimitSell => order.price.unwrap(),
            OrderType::MarketBuy => f64::MAX,
            OrderType::MarketSell => f64::MIN,
            _ => panic!("Can't fill cancel or modify"),
        };

        for bid in &depth.bids {
            let filled_size = filled.get_fill(&order.symbol, &bid.price);

            if let OrderBookOrderPriority::AlwaysFirst = priority_setting {
                if is_buy && bid.price == price_check {
                    if let Some((_buy_vol, sell_vol)) = taker_trades.get(&price_check.to_string()) {
                        let size = sell_vol - filled_size;
                        if size == 0.0 {
                            break;
                        }

                        let qty = if size >= to_fill { to_fill } else { size };
                        to_fill -= qty;

                        let trade = OrderResult {
                            symbol: order.symbol.clone(),
                            value: bid.price * qty,
                            quantity: qty,
                            date: depth.date,
                            typ: OrderResultType::Buy,
                            order_id: order.order_id,
                            order_id_ref: None,
                            exchange: order.exchange.clone(),
                        };

                        trades.push(trade);
                        filled.insert_fill(&order.symbol, &bid.price, qty);
                    }
                }
            }

            if !is_buy && bid.price >= price_check {
                let size = bid.size - filled_size;

                if size == 0.0 {
                    break;
                }

                let qty = if size >= to_fill { to_fill } else { size };
                to_fill -= qty;
                let trade = OrderResult {
                    symbol: order.symbol.clone(),
                    value: bid.price * qty,
                    quantity: qty,
                    date: depth.date,
                    typ: OrderResultType::Sell,
                    order_id: order.order_id,
                    order_id_ref: None,
                    exchange: order.exchange.clone(),
                };

                trades.push(trade);
                filled.insert_fill(&order.symbol, &bid.price, qty);

                if to_fill == 0.0 {
                    break;
                }
            }
        }

        for ask in &depth.asks {
            let filled_size = filled.get_fill(&order.symbol, &ask.price);

            if let OrderBookOrderPriority::AlwaysFirst = priority_setting {
                if !is_buy && ask.price == price_check {
                    if let Some((buy_vol, _sell_vol)) = taker_trades.get(&price_check.to_string()) {
                        let size = buy_vol - filled_size;
                        if size == 0.0 {
                            break;
                        }

                        let qty = if size >= to_fill { to_fill } else { size };
                        to_fill -= qty;

                        let trade = OrderResult {
                            symbol: order.symbol.clone(),
                            value: ask.price * qty,
                            quantity: qty,
                            date: depth.date,
                            typ: OrderResultType::Sell,
                            order_id: order.order_id,
                            order_id_ref: None,
                            exchange: order.exchange.clone(),
                        };

                        trades.push(trade);
                        filled.insert_fill(&order.symbol, &ask.price, qty);
                    }
                }
            }

            if is_buy && ask.price <= price_check {
                let filled_size = filled.get_fill(&order.symbol, &ask.price);
                let size = ask.size - filled_size;
                if size == 0.0 {
                    break;
                }

                let qty = if size >= to_fill { to_fill } else { size };
                to_fill -= qty;
                let trade = OrderResult {
                    symbol: order.symbol.clone(),
                    value: ask.price * qty,
                    quantity: qty,
                    date: depth.date,
                    typ: OrderResultType::Buy,
                    order_id: order.order_id,
                    order_id_ref: None,
                    exchange: order.exchange.clone(),
                };
                trades.push(trade);
                filled.insert_fill(&order.symbol, &ask.price, qty);

                if to_fill == 0.0 {
                    break;
                }
            }
        }
        trades
    }

    pub fn execute_orders(
        &mut self,
        quotes: &DateDepth,
        trades: &DateTrade,
        now: i64,
    ) -> Vec<OrderResult> {
        //Tracks liquidity that has been used at each level
        let mut filled: FillTracker = FillTracker::new();

        let mut trade_results = Vec::new();
        if self.is_empty() {
            return trade_results;
        }

        let mut taker_trades: FilledTrades = HashMap::new();
        for date_trades in trades.values() {
            for trade in date_trades {
                taker_trades
                    .entry(trade.px.to_string())
                    .or_insert_with(|| (0.0, 0.0));
                let volume = taker_trades.get_mut(&trade.px.to_string()).unwrap();
                match trade.side {
                    crate::source::hyperliquid::Side::Bid => volume.1 += trade.sz,
                    crate::source::hyperliquid::Side::Ask => volume.0 += trade.sz,
                }
            }
        }

        // Split out cancel and modifies, and then implement on a copy of orderbook
        let mut cancel_and_modify: Vec<InnerOrder> = Vec::new();
        let mut orders: BTreeMap<OrderId, InnerOrder> = BTreeMap::new();
        while let Some((order_id, order)) = self.inner.pop_first() {
            match order.order_type {
                OrderType::Cancel | OrderType::Modify => {
                    cancel_and_modify.push(order);
                }
                _ => {
                    orders.insert(order_id, order);
                }
            }
        }

        for order in cancel_and_modify {
            match order.order_type {
                OrderType::Cancel => {
                    let mut res = Self::cancel_order(now, &order, &mut orders);
                    if !res.is_empty() {
                        trade_results.append(&mut res);
                    }
                }
                OrderType::Modify => {
                    let mut res = Self::modify_order(now, &order, &mut orders);
                    if !res.is_empty() {
                        trade_results.append(&mut res);
                    }
                }
                _ => {}
            }
        }

        let mut unexecuted_orders = BTreeMap::new();
        while let Some((order_id, order)) = orders.pop_first() {
            let security_id = &order.symbol;

            if !self.latency.cmp_order(now, &order) {
                unexecuted_orders.insert(order_id, order);
                continue;
            }

            if let Some(exchange) = quotes.get(&order.exchange) {
                if let Some(depth) = exchange.get(security_id) {
                    let mut completed_trades = match order.order_type {
                        OrderType::MarketBuy => Self::fill_order(
                            depth,
                            &order,
                            &mut filled,
                            &taker_trades,
                            &self.priority_setting,
                        ),
                        OrderType::MarketSell => Self::fill_order(
                            depth,
                            &order,
                            &mut filled,
                            &taker_trades,
                            &self.priority_setting,
                        ),
                        OrderType::LimitBuy => Self::fill_order(
                            depth,
                            &order,
                            &mut filled,
                            &taker_trades,
                            &self.priority_setting,
                        ),
                        OrderType::LimitSell => Self::fill_order(
                            depth,
                            &order,
                            &mut filled,
                            &taker_trades,
                            &self.priority_setting,
                        ),
                        // There shouldn't be any cancel or modifies by this point
                        _ => vec![],
                    };

                    if completed_trades.is_empty() {
                        unexecuted_orders.insert(order_id, order);
                    }

                    trade_results.append(&mut completed_trades)
                }
            } else {
                unexecuted_orders.insert(order_id, order);
            }
        }
        self.inner = unexecuted_orders;
        trade_results
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::{
        exchange::uist_v2::{Order, OrderBook},
        source::hyperliquid::{DateDepth, DateTrade, Depth, Level, Side, Trade},
    };

    fn trades() -> DateTrade {
        let bid_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Bid,
            px: 100.0,
            sz: 100.0,
            time: 100,
            exchange: "exchange".to_string(),
        };
        let ask_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Ask,
            px: 102.0,
            sz: 100.0,
            time: 100,
            exchange: "exchange".to_string(),
        };

        let mut trades: DateTrade = BTreeMap::new();
        trades.insert(100, vec![bid_trade, ask_trade]);

        trades
    }

    fn quotes() -> DateDepth {
        let bid_level = Level {
            price: 100.0,
            size: 100.0,
        };

        let ask_level = Level {
            price: 102.0,
            size: 100.0,
        };

        let mut depth = Depth::new(100, "ABC", "exchange");
        depth.add_level(bid_level, Side::Bid);
        depth.add_level(ask_level, Side::Ask);

        let mut quotes: DateDepth = BTreeMap::new();
        quotes.insert("exchange".to_string(), BTreeMap::new());
        quotes
            .get_mut("exchange")
            .unwrap()
            .insert("ABC".to_string(), depth);
        quotes
    }

    fn trades1() -> DateTrade {
        let bid_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Bid,
            px: 98.0,
            sz: 20.0,
            time: 100,
            exchange: "exchange".to_string(),
        };
        let ask_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Ask,
            px: 102.0,
            sz: 20.0,
            time: 100,
            exchange: "exchange".to_string(),
        };

        let mut trades: DateTrade = BTreeMap::new();
        trades.insert(100, vec![bid_trade, ask_trade]);

        trades
    }

    fn quotes1() -> DateDepth {
        let bid_level = Level {
            price: 98.0,
            size: 20.0,
        };

        let ask_level = Level {
            price: 102.0,
            size: 20.0,
        };

        let mut depth = Depth::new(100, "ABC", "exchange");
        depth.add_level(bid_level, Side::Bid);
        depth.add_level(ask_level, Side::Ask);

        let mut quotes: DateDepth = BTreeMap::new();
        quotes.insert("exchange".to_string(), BTreeMap::new());
        quotes
            .get_mut("exchange")
            .unwrap()
            .insert("ABC".to_string(), depth);
        quotes
    }

    #[test]
    fn test_that_nonexistent_buy_order_cancel_produces_empty_result() {
        let quotes = quotes();
        let trades = trades();
        let mut orderbook = OrderBook::new();
        orderbook.insert_order(Order::cancel_order("ABC", 10, "exchange"), 100);
        let res = orderbook.execute_orders(&quotes, &trades, 100);
        assert!(res.is_empty())
    }

    #[test]
    fn test_that_nonexistent_buy_order_modify_throws_error() {
        let quotes = quotes();
        let trades = trades();
        let mut orderbook = OrderBook::new();
        orderbook.insert_order(Order::modify_order("ABC", 10, 100.0, "exchange"), 100);
        let res = orderbook.execute_orders(&quotes, &trades, 100);
        assert!(res.is_empty())
    }

    #[test]
    fn test_that_buy_order_can_be_cancelled_and_modified() {
        let quotes = quotes();
        let trades = trades();

        let mut orderbook = OrderBook::new();
        let oid = orderbook
            .insert_order(Order::limit_buy("ABC", 100.0, 1.0, "exchange"), 100)
            .order_id;

        orderbook.insert_order(Order::cancel_order("ABC", oid, "exchange"), 100);
        let res = orderbook.execute_orders(&quotes, &trades, 100);
        println!("{:?}", res);
        assert!(res.len() == 1);

        let oid1 = orderbook
            .insert_order(Order::limit_buy("ABC", 200.0, 1.0, "exchange"), 100)
            .order_id;
        orderbook.insert_order(Order::modify_order("ABC", oid1, 100.0, "exchange"), 100);
        let res = orderbook.execute_orders(&quotes, &trades, 100);
        assert!(res.len() == 1);
    }

    #[test]
    fn test_that_buy_order_will_lift_all_volume_when_order_is_equal_to_depth_size() {
        let quotes = quotes();
        let trades = trades();

        let mut orderbook = OrderBook::new();
        let order = Order::market_buy("ABC", 100.0, "exchange");
        orderbook.insert_order(order, 100);

        let res = orderbook.execute_orders(&quotes, &trades, 100);
        assert!(res.len() == 1);
        let trade = res.first().unwrap();
        assert!(trade.quantity == 100.00);
        assert!(trade.value / trade.quantity == 102.00);
    }

    #[test]
    fn test_that_sell_order_will_lift_all_volume_when_order_is_equal_to_depth_size() {
        let quotes = quotes();
        let trades = trades();

        let mut orderbook = OrderBook::new();
        let order = Order::market_sell("ABC", 100.0, "exchange");
        orderbook.insert_order(order, 100);

        let res = orderbook.execute_orders(&quotes, &trades, 100);
        assert!(res.len() == 1);
        let trade = res.first().unwrap();
        assert!(trade.quantity == 100.00);
        assert!(trade.value / trade.quantity == 100.00);
    }

    #[test]
    fn test_that_order_will_lift_order_qty_when_order_is_less_than_depth_size() {
        let quotes = quotes();
        let trades = trades();
        let mut orderbook = OrderBook::new();
        let order = Order::market_buy("ABC", 50.0, "exchange");
        orderbook.insert_order(order, 100);

        let res = orderbook.execute_orders(&quotes, &trades, 100);
        assert!(res.len() == 1);
        let trade = res.first().unwrap();
        assert!(trade.quantity == 50.00);
        assert!(trade.value / trade.quantity == 102.00);
    }

    #[test]
    fn test_that_order_will_lift_qty_from_other_levels_when_price_is_good() {
        let bid_level = Level {
            price: 100.0,
            size: 100.0,
        };

        let ask_level = Level {
            price: 102.0,
            size: 80.0,
        };

        let ask_level_1 = Level {
            price: 103.0,
            size: 20.0,
        };

        let mut depth = Depth::new(100, "ABC", "exchange");
        depth.add_level(bid_level, Side::Bid);
        depth.add_level(ask_level, Side::Ask);
        depth.add_level(ask_level_1, Side::Ask);

        let mut quotes: DateDepth = BTreeMap::new();
        quotes.insert("exchange".to_string(), BTreeMap::new());
        quotes
            .get_mut("exchange")
            .unwrap()
            .insert("ABC".to_string(), depth);

        let bid_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Bid,
            px: 100.0,
            sz: 100.0,
            time: 100,
            exchange: "exchange".to_string(),
        };
        let ask_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Ask,
            px: 102.0,
            sz: 80.0,
            time: 100,
            exchange: "exchange".to_string(),
        };

        let mut trades: DateTrade = BTreeMap::new();
        trades.insert(100, vec![bid_trade, ask_trade]);

        let mut orderbook = OrderBook::new();
        let order = Order::market_buy("ABC", 100.0, "exchange");
        orderbook.insert_order(order, 100);

        let res = orderbook.execute_orders(&quotes, &trades, 100);
        assert!(res.len() == 2);
        let first_trade = res.first().unwrap();
        let second_trade = res.get(1).unwrap();

        println!("{:?}", first_trade);
        println!("{:?}", second_trade);
        assert!(first_trade.quantity == 80.0);
        assert!(second_trade.quantity == 20.0);
    }

    #[test]
    fn test_that_limit_buy_order_lifts_all_volume_when_price_is_good() {
        let bid_level = Level {
            price: 100.0,
            size: 100.0,
        };

        let ask_level = Level {
            price: 102.0,
            size: 80.0,
        };

        let ask_level_1 = Level {
            price: 103.0,
            size: 20.0,
        };

        let ask_level_2 = Level {
            price: 104.0,
            size: 20.0,
        };

        let mut depth = Depth::new(100, "ABC", "exchange");
        depth.add_level(bid_level, Side::Bid);
        depth.add_level(ask_level, Side::Ask);
        depth.add_level(ask_level_1, Side::Ask);
        depth.add_level(ask_level_2, Side::Ask);

        let mut quotes: DateDepth = BTreeMap::new();
        quotes.insert("exchange".to_string(), BTreeMap::new());
        quotes
            .get_mut("exchange")
            .unwrap()
            .insert("ABC".to_string(), depth);

        let bid_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Bid,
            px: 100.0,
            sz: 100.0,
            time: 100,
            exchange: "exchange".to_string(),
        };
        let ask_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Ask,
            px: 102.0,
            sz: 80.0,
            time: 100,
            exchange: "exchange".to_string(),
        };

        let mut trades: DateTrade = BTreeMap::new();
        trades.insert(100, vec![bid_trade, ask_trade]);

        let mut orderbook = OrderBook::new();
        let order = Order::limit_buy("ABC", 120.0, 103.00, "exchange");
        orderbook.insert_order(order, 100);

        let res = orderbook.execute_orders(&quotes, &trades, 100);
        println!("{:?}", res);
        assert!(res.len() == 2);
        let first_trade = res.first().unwrap();
        let second_trade = res.get(1).unwrap();

        println!("{:?}", first_trade);
        println!("{:?}", second_trade);
        assert!(first_trade.quantity == 80.0);
        assert!(second_trade.quantity == 20.0);
    }

    #[test]
    fn test_that_limit_sell_order_lifts_all_volume_when_price_is_good() {
        let bid_level_0 = Level {
            price: 98.0,
            size: 20.0,
        };

        let bid_level_1 = Level {
            price: 99.0,
            size: 20.0,
        };

        let bid_level_2 = Level {
            price: 100.0,
            size: 80.0,
        };

        let ask_level = Level {
            price: 102.0,
            size: 80.0,
        };

        let mut depth = Depth::new(100, "ABC", "exchange");
        depth.add_level(bid_level_0, Side::Bid);
        depth.add_level(bid_level_1, Side::Bid);
        depth.add_level(bid_level_2, Side::Bid);
        depth.add_level(ask_level, Side::Ask);

        let mut quotes: DateDepth = BTreeMap::new();
        quotes.insert("exchange".to_string(), BTreeMap::new());
        quotes
            .get_mut("exchange")
            .unwrap()
            .insert("ABC".to_string(), depth);

        let bid_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Bid,
            px: 100.0,
            sz: 80.0,
            time: 100,
            exchange: "exchange".to_string(),
        };
        let ask_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Ask,
            px: 102.0,
            sz: 80.0,
            time: 100,
            exchange: "exchange".to_string(),
        };

        let mut trades: DateTrade = BTreeMap::new();
        trades.insert(100, vec![bid_trade, ask_trade]);

        let mut orderbook = OrderBook::new();
        let order = Order::limit_sell("ABC", 120.0, 99.00, "exchange");
        orderbook.insert_order(order, 100);

        let res = orderbook.execute_orders(&quotes, &trades, 100);
        println!("{:?}", res);
        assert!(res.len() == 2);
        let first_trade = res.first().unwrap();
        let second_trade = res.get(1).unwrap();

        println!("{:?}", first_trade);
        println!("{:?}", second_trade);
        assert!(first_trade.quantity == 80.0);
        assert!(second_trade.quantity == 20.0);
    }

    #[test]
    fn test_that_repeated_orders_do_not_use_same_liquidty() {
        let quotes = quotes1();
        let trades = trades1();
        let mut orderbook = OrderBook::new();
        let first_order = Order::limit_buy("ABC", 20.0, 103.00, "exchange");
        orderbook.insert_order(first_order, 100);
        let second_order = Order::limit_buy("ABC", 20.0, 103.00, "exchange");
        orderbook.insert_order(second_order, 100);

        let res = orderbook.execute_orders(&quotes, &trades, 100);
        println!("{:?}", res);
        assert!(res.len() == 1);
    }

    #[test]
    fn test_that_latency_model_filters_orders() {
        let bid_level = Level {
            price: 98.0,
            size: 20.0,
        };

        let ask_level = Level {
            price: 102.0,
            size: 20.0,
        };

        let mut depth = Depth::new(100, "ABC", "exchange");
        depth.add_level(bid_level.clone(), Side::Bid);
        depth.add_level(ask_level.clone(), Side::Ask);

        let mut depth_101 = Depth::new(101, "ABC", "exchange");
        depth_101.add_level(bid_level.clone(), Side::Bid);
        depth_101.add_level(ask_level.clone(), Side::Ask);

        let mut depth_102 = Depth::new(102, "ABC", "exchange");
        depth_102.add_level(bid_level, Side::Bid);
        depth_102.add_level(ask_level, Side::Ask);

        let mut quotes: DateDepth = BTreeMap::new();
        quotes.insert("exchange".to_string(), BTreeMap::new());
        let exchange = quotes.get_mut("exchange").unwrap();
        exchange.insert("ABC".to_string(), depth);
        exchange.insert("ABC".to_string(), depth_101);
        exchange.insert("ABC".to_string(), depth_102);

        let bid_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Bid,
            px: 98.0,
            sz: 20.0,
            time: 100,
            exchange: "exchange".to_string(),
        };
        let ask_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Ask,
            px: 102.0,
            sz: 20.0,
            time: 100,
            exchange: "exchange".to_string(),
        };

        let mut trades: DateTrade = BTreeMap::new();
        trades.insert(100, vec![bid_trade, ask_trade]);

        let mut orderbook = OrderBook::with_latency(1);
        let order = Order::limit_buy("ABC", 20.0, 103.00, "exchange");
        orderbook.insert_order(order, 100);

        let trades_100 = orderbook.execute_orders(&quotes, &trades, 100);
        let trades_101 = orderbook.execute_orders(&quotes, &trades, 101);
        let trades_102 = orderbook.execute_orders(&quotes, &trades, 102);

        println!("{:?}", trades_101);

        assert!(trades_100.is_empty());
        assert!(trades_101.is_empty());
        assert!(trades_102.len() == 1);
    }

    #[test]
    fn test_that_orderbook_clears_after_execution() {
        let quotes = quotes1();
        let trades = trades1();
        let mut orderbook = OrderBook::new();
        let order = Order::market_buy("ABC", 20.0, "exchange");
        orderbook.insert_order(order, 100);

        let completed_trades = orderbook.execute_orders(&quotes, &trades, 100);
        let completed_trades1 = orderbook.execute_orders(&quotes, &trades, 101);

        assert!(completed_trades.len() == 1);
        assert!(completed_trades1.is_empty());
    }

    #[test]
    fn test_that_order_id_is_incrementing_and_unique() {
        let bid_level = Level {
            price: 98.0,
            size: 20.0,
        };

        let ask_level = Level {
            price: 102.0,
            size: 20.0,
        };

        let mut depth = Depth::new(100, "ABC", "exchange");
        depth.add_level(bid_level.clone(), Side::Bid);
        depth.add_level(ask_level.clone(), Side::Ask);

        let mut depth_101 = Depth::new(101, "ABC", "exchange");
        depth_101.add_level(bid_level.clone(), Side::Bid);
        depth_101.add_level(ask_level.clone(), Side::Ask);

        let mut depth_102 = Depth::new(102, "ABC", "exchange");
        depth_102.add_level(bid_level, Side::Bid);
        depth_102.add_level(ask_level, Side::Ask);

        let mut quotes: DateDepth = BTreeMap::new();
        quotes.insert("exchange".to_string(), BTreeMap::new());
        let exchange = quotes.get_mut("exchange").unwrap();
        exchange.insert("ABC".to_string(), depth);
        exchange.insert("ABC".to_string(), depth_101);
        exchange.insert("ABC".to_string(), depth_102);

        let bid_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Bid,
            px: 98.0,
            sz: 20.0,
            time: 100,
            exchange: "exchange".to_string(),
        };
        let ask_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Ask,
            px: 102.0,
            sz: 20.0,
            time: 100,
            exchange: "exchange".to_string(),
        };

        let mut trades: DateTrade = BTreeMap::new();
        trades.insert(100, vec![bid_trade, ask_trade]);

        let mut orderbook = OrderBook::new();
        let order = Order::limit_buy("ABC", 20.0, 103.00, "exchange");
        let order1 = Order::limit_buy("ABC", 20.0, 103.00, "exchange");
        let order2 = Order::limit_buy("ABC", 20.0, 103.00, "exchange");

        let res = orderbook.insert_order(order, 100);
        let res1 = orderbook.insert_order(order1, 100);
        let _ = orderbook.execute_orders(&quotes, &trades, 100);

        let res2 = orderbook.insert_order(order2, 101);
        let _ = orderbook.execute_orders(&quotes, &trades, 101);

        assert!(res.order_id == 0);
        assert!(res1.order_id == 1);
        assert!(res2.order_id == 2);
    }

    #[test]
    fn test_that_volume_lifts_with_trades_inside() {
        let bid_level = Level {
            price: 98.0,
            size: 100.0,
        };

        let ask_level = Level {
            price: 102.0,
            size: 100.0,
        };

        let mut depth = Depth::new(100, "ABC", "exchange");
        depth.add_level(bid_level.clone(), Side::Bid);
        depth.add_level(ask_level.clone(), Side::Ask);

        let mut quotes: DateDepth = BTreeMap::new();
        quotes.insert("exchange".to_string(), BTreeMap::new());
        quotes
            .get_mut("exchange")
            .unwrap()
            .insert("ABC".to_string(), depth);

        let bid_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Bid,
            px: 98.0,
            sz: 20.0,
            time: 100,
            exchange: "exchange".to_string(),
        };
        let ask_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Ask,
            px: 102.0,
            sz: 20.0,
            time: 100,
            exchange: "exchange".to_string(),
        };

        let mut trades: DateTrade = BTreeMap::new();
        trades.insert(100, vec![bid_trade, ask_trade]);

        let mut orderbook = OrderBook::new();
        let buy_order = Order::limit_buy("ABC", 10.0, 98.00, "exchange");
        orderbook.insert_order(buy_order, 99);
        let sell_order = Order::limit_sell("ABC", 10.0, 102.00, "exchange");
        orderbook.insert_order(sell_order, 99);

        let res = orderbook.execute_orders(&quotes, &trades, 100);
        assert!(res.len() == 2);
    }

    #[test]
    fn test_that_fills_only_traded_volume_on_inside() {
        let bid_level = Level {
            price: 98.0,
            size: 100.0,
        };

        let ask_level = Level {
            price: 102.0,
            size: 100.0,
        };

        let mut depth = Depth::new(100, "ABC", "exchange");
        depth.add_level(bid_level.clone(), Side::Bid);
        depth.add_level(ask_level.clone(), Side::Ask);

        let mut quotes: DateDepth = BTreeMap::new();
        quotes.insert("exchange".to_string(), BTreeMap::new());
        quotes
            .get_mut("exchange")
            .unwrap()
            .insert("ABC".to_string(), depth);

        let bid_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Bid,
            px: 98.0,
            sz: 20.0,
            time: 100,
            exchange: "exchange".to_string(),
        };
        let ask_trade = Trade {
            coin: "ABC".to_string(),
            side: Side::Ask,
            px: 102.0,
            sz: 20.0,
            time: 100,
            exchange: "exchange".to_string(),
        };

        let mut trades: DateTrade = BTreeMap::new();
        trades.insert(100, vec![bid_trade, ask_trade]);

        let mut orderbook = OrderBook::new();
        let buy_order = Order::limit_buy("ABC", 40.0, 98.00, "exchange");
        orderbook.insert_order(buy_order, 99);
        let sell_order = Order::limit_sell("ABC", 40.0, 102.00, "exchange");
        orderbook.insert_order(sell_order, 99);

        let res = orderbook.execute_orders(&quotes, &trades, 100);
        assert!(res.len() == 2);
        assert!(res.first().unwrap().quantity == 20.0);
        assert!(res.get(1).unwrap().quantity == 20.0);
    }
}
