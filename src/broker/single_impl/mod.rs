mod builder;
pub use builder::SingleBrokerBuilder;

use log::info;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::exchange::SingleExchange;
use crate::input::{DataSource, Dividendable, Quotable};
use crate::types::{CashValue, PortfolioHoldings, PortfolioQty, Price};

use super::{
    BacktestBroker, BrokerCalculations, BrokerCashEvent, BrokerCost, BrokerEvent, BrokerLog,
    DividendPayment, EventLog, GetsQuote, Order, OrderType, ReceievesOrders, Trade, TransferCash,
};

#[derive(Debug)]
pub struct SingleBroker<T, Q, D>
where
    Q: Quotable,
    D: Dividendable,
    T: DataSource<Q, D>,
{
    //We have overlapping functionality because we are storing
    data: T,
    //TODO: this could be preallocated, tiny gains but this can only be as large as
    //the number of stocks in the universe. If we have a lot of changes then the HashMap
    //will be constantly resized.
    holdings: PortfolioHoldings,
    cash: CashValue,
    log: BrokerLog,
    trade_costs: Vec<BrokerCost>,
    last_seen_trade: usize,
    latest_quotes: HashMap<String, Arc<Q>>,
    exchange: SingleExchange<T, Q, D>,
    _dividend: PhantomData<D>,
}

impl<T, Q, D> SingleBroker<T, Q, D>
where
    Q: Quotable,
    D: Dividendable,
    T: DataSource<Q, D>,
{
    pub fn cost_basis(&self, symbol: &str) -> Option<Price> {
        self.log.cost_basis(symbol)
    }

    //Contains tasks that should be run on every iteration of the simulation irregardless of the
    //state on the client.
    pub fn check(&mut self) {
        self.pay_dividends();
        self.exchange.check();

        //Update prices, these prices are not tradable
        for quote in &self.exchange.fetch_quotes() {
            self.latest_quotes
                .insert(quote.get_symbol().to_string(), Arc::clone(quote));
        }

        //Reconcile broker against executed trades
        let completed_trades = self.exchange.fetch_trades(self.last_seen_trade).to_owned();
        for trade in completed_trades {
            match trade.typ {
                //Force debit so we can end up with negative cash here
                crate::exchange::TradeType::Buy => self.debit_force(&trade.value),
                crate::exchange::TradeType::Sell => self.credit(&trade.value),
            };
            self.log.record::<Trade>(trade.clone().into());

            let default = PortfolioQty::from(0.0);
            let curr_position = self.get_position_qty(&trade.symbol).unwrap_or(&default);

            let updated = match trade.typ {
                crate::exchange::TradeType::Buy => **curr_position + trade.quantity,
                crate::exchange::TradeType::Sell => **curr_position - trade.quantity,
            };

            self.update_holdings(&trade.symbol, PortfolioQty::from(updated));
            self.last_seen_trade += 1;
        }
        //Previous step can cause negative cash balance so we have to rebalance here, this
        //is not instant so will never balance properly if the series is very volatile
        self.rebalance_cash();
    }

    fn rebalance_cash(&mut self) {
        //Has to be less than, we can have zero value without needing to liquidate if we initialize
        //the portfolio but exchange doesn't execute any trades. This can happen if we are missing
        //prices at the start of the series
        if *self.cash < 0.0 {
            let shortfall = *self.cash * -1.0;
            //When we raise cash, we try to raise a small amount more to stop continuous
            //rebalancing, this amount is arbitrary atm
            let plus_buffer = shortfall + 1000.0;

            let _res = BrokerCalculations::withdraw_cash_with_liquidation(&plus_buffer, self);
            //TODO: handle insufficient cash state
        }
    }
}

impl<T, Q, D> GetsQuote<Q, D> for SingleBroker<T, Q, D>
where
    Q: Quotable,
    D: Dividendable,
    T: DataSource<Q, D>,
{
    fn get_quote(&self, symbol: &str) -> Option<Arc<Q>> {
        self.latest_quotes.get(symbol).cloned()
    }

    fn get_quotes(&self) -> Option<Vec<Arc<Q>>> {
        if self.latest_quotes.is_empty() {
            return None;
        }

        let mut tmp = Vec::new();
        for quote in self.latest_quotes.values() {
            tmp.push(Arc::clone(quote));
        }
        Some(tmp)
    }
}

impl<T, Q, D> BacktestBroker for SingleBroker<T, Q, D>
where
    Q: Quotable,
    D: Dividendable,
    T: DataSource<Q, D>,
{
    //Identical to deposit_cash but is seperated to distinguish internal cash
    //transactions from external with no value returned to client
    fn credit(&mut self, value: &f64) -> BrokerCashEvent {
        info!(
            "BROKER: Credited {:?} cash, current balance of {:?}",
            value, self.cash
        );
        self.cash = CashValue::from(*value + *self.cash);
        BrokerCashEvent::DepositSuccess(CashValue::from(*value))
    }

    //Looks similar to withdraw_cash but distinguished because it represents
    //failure of an internal transaction with no value returned to clients
    fn debit(&mut self, value: &f64) -> BrokerCashEvent {
        if value > &self.cash {
            info!(
                "BROKER: Debit failed of {:?} cash, current balance of {:?}",
                value, self.cash
            );
            return BrokerCashEvent::WithdrawFailure(CashValue::from(*value));
        }
        info!(
            "BROKER: Debited {:?} cash, current balance of {:?}",
            value, self.cash
        );
        self.cash = CashValue::from(*self.cash - *value);
        BrokerCashEvent::WithdrawSuccess(CashValue::from(*value))
    }

    fn debit_force(&mut self, value: &f64) -> BrokerCashEvent {
        info!(
            "BROKER: Force debt {:?} cash, current balance of {:?}",
            value, self.cash
        );
        self.cash = CashValue::from(*self.cash - *value);
        BrokerCashEvent::WithdrawSuccess(CashValue::from(*value))
    }

    fn get_cash_balance(&self) -> CashValue {
        self.cash.clone()
    }

    //This method used to mut because we needed to sort last prices on the broker, this has now
    //been moved to the exchange. The exchange is responsible for storing last prices for cases
    //when a quote is missing.
    fn get_position_value(&self, symbol: &str) -> Option<CashValue> {
        //TODO: we need to introduce some kind of distinction between short and long
        //      positions.

        if let Some(quote) = self.get_quote(symbol) {
            //We only have long positions so we only need to look at the bid
            let price = quote.get_bid();
            if let Some(qty) = self.get_position_qty(symbol) {
                let val = **price * **qty;
                return Some(CashValue::from(val));
            }
        }
        //This should only occur in cases when the client erroneously asks for a security with no
        //current or historical prices, which should never happen for a security in the portfolio.
        //This path likely represent an error in the application code so may panic here in future.
        None
    }

    fn get_position_cost(&self, symbol: &str) -> Option<Price> {
        self.log.cost_basis(symbol)
    }

    fn get_position_qty(&self, symbol: &str) -> Option<&PortfolioQty> {
        self.holdings.get(symbol)
    }

    fn get_positions(&self) -> Vec<String> {
        self.holdings.keys()
    }

    fn get_holdings(&self) -> PortfolioHoldings {
        self.holdings.clone()
    }

    fn update_holdings(&mut self, symbol: &str, change: PortfolioQty) {
        //We have to take ownership for logging but it is easier just to use ref for symbol as that
        //is used throughout
        let symbol_own = symbol.to_string();
        info!(
            "BROKER: Incrementing holdings in {:?} by {:?}",
            symbol_own, change
        );
        if (*change).eq(&0.0) {
            self.holdings.remove(symbol.as_ref());
        } else {
            self.holdings.insert(symbol.as_ref(), &change);
        }
    }

    fn get_trade_costs(&self, trade: &Trade) -> CashValue {
        let mut cost = CashValue::default();
        for trade_cost in &self.trade_costs {
            cost = CashValue::from(*cost + *trade_cost.calc(trade));
        }
        cost
    }

    fn calc_trade_impact(&self, budget: &f64, price: &f64, is_buy: bool) -> (CashValue, Price) {
        BrokerCost::trade_impact_total(&self.trade_costs, budget, price, is_buy)
    }

    fn pay_dividends(&mut self) {
        info!("BROKER: Checking dividends");
        let mut dividend_value: CashValue = CashValue::from(0.0);
        if let Some(dividends) = self.data.get_dividends() {
            for dividend in dividends.iter() {
                //Our dataset can include dividends for stocks we don't own so we need to check
                //that we own the stock, not performant but can be changed later
                if let Some(qty) = self.get_position_qty(dividend.get_symbol()) {
                    info!(
                        "BROKER: Found dividend of {:?} for portfolio holding {:?}",
                        dividend.get_value(),
                        dividend.get_symbol()
                    );
                    let cash_value = CashValue::from(*qty.clone() * **dividend.get_value());
                    dividend_value = cash_value.clone() + dividend_value;
                    let dividend_paid = DividendPayment::new(
                        cash_value,
                        dividend.get_symbol().clone(),
                        *dividend.get_date(),
                    );
                    self.log.record(dividend_paid);
                }
            }
        }
        self.credit(&dividend_value);
    }
}

impl<T, Q, D> ReceievesOrders for SingleBroker<T, Q, D>
where
    Q: Quotable,
    D: Dividendable,
    T: DataSource<Q, D>,
{
    fn send_order(&mut self, order: Order) -> BrokerEvent {
        //This is an estimate of the cost based on the current price, can still end with negative
        //balance when we reconcile with actuals, may also reject valid orders at the margin
        info!(
            "BROKER: Attempting to send {:?} order for {:?} shares of {:?} to the exchange",
            order.get_order_type(),
            order.get_shares(),
            order.get_symbol()
        );

        let quote = self.get_quote(order.get_symbol()).unwrap();
        let price = match order.get_order_type() {
            OrderType::MarketBuy | OrderType::LimitBuy | OrderType::StopBuy => quote.get_ask(),
            OrderType::MarketSell | OrderType::LimitSell | OrderType::StopSell => quote.get_bid(),
        };

        if let Err(_err) = BrokerCalculations::client_has_sufficient_cash(&order, price, self) {
            info!(
                "BROKER: Unable to send {:?} order for {:?} shares of {:?} to exchange",
                order.get_order_type(),
                order.get_shares(),
                order.get_symbol()
            );
            return BrokerEvent::OrderInvalid(order.clone());
        }
        if let Err(_err) = BrokerCalculations::client_has_sufficient_holdings_for_sale(&order, self)
        {
            info!(
                "BROKER: Unable to send {:?} order for {:?} shares of {:?} to exchange",
                order.get_order_type(),
                order.get_shares(),
                order.get_symbol()
            );
            return BrokerEvent::OrderInvalid(order.clone());
        }
        if let Err(_err) = BrokerCalculations::client_is_issuing_nonsense_order(&order) {
            info!(
                "BROKER: Unable to send {:?} order for {:?} shares of {:?} to exchange",
                order.get_order_type(),
                order.get_shares(),
                order.get_symbol()
            );
            return BrokerEvent::OrderInvalid(order.clone());
        }

        self.exchange.insert_order(order.into_exchange(0));

        info!(
            "BROKER: Successfully sent {:?} order for {:?} shares of {:?} to exchange",
            order.get_order_type(),
            order.get_shares(),
            order.get_symbol()
        );
        BrokerEvent::OrderSentToExchange(order)
    }

    fn send_orders(&mut self, orders: &[Order]) -> Vec<BrokerEvent> {
        let mut res = Vec::new();
        for o in orders {
            let trade = self.send_order(o.clone());
            res.push(trade);
        }
        res
    }
}

impl<T, Q, D> TransferCash for SingleBroker<T, Q, D>
where
    Q: Quotable,
    D: Dividendable,
    T: DataSource<Q, D>,
{
}

impl<T, Q, D> EventLog for SingleBroker<T, Q, D>
where
    Q: Quotable,
    D: Dividendable,
    T: DataSource<Q, D>,
{
    fn trades_between(&self, start: &i64, end: &i64) -> Vec<Trade> {
        self.log.trades_between(start, end)
    }

    fn dividends_between(&self, start: &i64, end: &i64) -> Vec<DividendPayment> {
        self.log.dividends_between(start, end)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::broker::{
        BacktestBroker, BrokerCashEvent, BrokerCost, BrokerEvent, Dividend, Order, OrderType,
        Quote, ReceievesOrders, TransferCash,
    };
    use crate::exchange::SingleExchangeBuilder;
    use crate::input::{HashMapInput, HashMapInputBuilder};
    use crate::types::DateTime;

    use super::{SingleBroker, SingleBrokerBuilder};

    fn setup() -> SingleBroker<HashMapInput, Quote, Dividend> {
        let mut prices: HashMap<DateTime, Vec<Arc<Quote>>> = HashMap::new();
        let mut dividends: HashMap<DateTime, Vec<Arc<Dividend>>> = HashMap::new();
        let quote = Arc::new(Quote::new(100.00, 101.00, 100, "ABC"));
        let quote1 = Arc::new(Quote::new(10.00, 11.00, 100, "BCD"));
        let quote2 = Arc::new(Quote::new(104.00, 105.00, 101, "ABC"));
        let quote3 = Arc::new(Quote::new(14.00, 15.00, 101, "BCD"));
        let quote4 = Arc::new(Quote::new(95.00, 96.00, 102, "ABC"));
        let quote5 = Arc::new(Quote::new(10.00, 11.00, 102, "BCD"));
        let quote6 = Arc::new(Quote::new(95.00, 96.00, 103, "ABC"));
        let quote7 = Arc::new(Quote::new(10.00, 11.00, 103, "BCD"));

        prices.insert(100.into(), vec![quote, quote1]);
        prices.insert(101.into(), vec![quote2, quote3]);
        prices.insert(102.into(), vec![quote4, quote5]);
        prices.insert(103.into(), vec![quote6, quote7]);

        let divi1 = Arc::new(Dividend::new(5.0, "ABC", 102));
        dividends.insert(102.into(), vec![divi1]);

        let clock = crate::clock::ClockBuilder::with_length_in_seconds(100, 5)
            .with_frequency(&crate::types::Frequency::Second)
            .build();

        let source = crate::input::HashMapInputBuilder::new()
            .with_clock(clock.clone())
            .with_quotes(prices)
            .with_dividends(dividends)
            .build();

        let exchange = SingleExchangeBuilder::new()
            .with_clock(clock.clone())
            .with_data_source(source.clone())
            .build();

        let brkr = SingleBrokerBuilder::new()
            .with_data(source)
            .with_exchange(exchange)
            .with_trade_costs(vec![BrokerCost::PctOfValue(0.01)])
            .build();

        brkr
    }

    #[test]
    fn test_cash_deposit_withdraw() {
        let mut brkr = setup();
        brkr.deposit_cash(&100.0);

        brkr.check();

        //Test cash
        assert!(matches!(
            brkr.withdraw_cash(&50.0),
            BrokerCashEvent::WithdrawSuccess(..)
        ));
        assert!(matches!(
            brkr.withdraw_cash(&51.0),
            BrokerCashEvent::WithdrawFailure(..)
        ));
        assert!(matches!(
            brkr.deposit_cash(&50.0),
            BrokerCashEvent::DepositSuccess(..)
        ));

        //Test transactions
        assert!(matches!(
            brkr.debit(&50.0),
            BrokerCashEvent::WithdrawSuccess(..)
        ));
        assert!(matches!(
            brkr.debit(&51.0),
            BrokerCashEvent::WithdrawFailure(..)
        ));
        assert!(matches!(
            brkr.credit(&50.0),
            BrokerCashEvent::DepositSuccess(..)
        ));
    }

    #[test]
    fn test_that_buy_order_reduces_cash_and_increases_holdings() {
        let mut brkr = setup();
        brkr.deposit_cash(&100_000.0);

        let res = brkr.send_order(Order::market(OrderType::MarketBuy, "ABC", 495.0));
        println!("{:?}", res);
        assert!(matches!(res, BrokerEvent::OrderSentToExchange(..)));

        brkr.check();

        let cash = brkr.get_cash_balance();
        assert!(*cash < 100_000.0);

        let qty = brkr.get_position_qty("ABC").unwrap();
        assert_eq!(*qty.clone(), 495.00);
    }

    #[test]
    fn test_that_buy_order_larger_than_cash_fails_with_error_returned_without_panic() {
        let mut brkr = setup();
        brkr.deposit_cash(&100.0);
        //Order value is greater than cash balance
        let res = brkr.send_order(Order::market(OrderType::MarketBuy, "ABC", 495.0));

        assert!(matches!(res, BrokerEvent::OrderInvalid(..)));
        brkr.check();

        let cash = brkr.get_cash_balance();
        assert!(*cash == 100.0);
    }

    #[test]
    fn test_that_sell_order_larger_than_holding_fails_with_error_returned_without_panic() {
        let mut brkr = setup();
        brkr.deposit_cash(&100_000.0);
        let res = brkr.send_order(Order::market(OrderType::MarketBuy, "ABC", 100.0));
        assert!(matches!(res, BrokerEvent::OrderSentToExchange(..)));
        brkr.check();

        //Order greater than current holding
        brkr.check();
        let res = brkr.send_order(Order::market(OrderType::MarketSell, "ABC", 105.0));
        assert!(matches!(res, BrokerEvent::OrderInvalid(..)));

        //Checking that
        let qty = brkr.get_position_qty("ABC").unwrap();
        println!("{:?}", qty);
        assert!((*qty.clone()).eq(&100.0));
    }

    #[test]
    fn test_that_market_sell_increases_cash_and_decreases_holdings() {
        let mut brkr = setup();
        brkr.deposit_cash(&100_000.0);
        let res = brkr.send_order(Order::market(OrderType::MarketBuy, "ABC", 495.0));
        assert!(matches!(res, BrokerEvent::OrderSentToExchange(..)));
        brkr.check();
        let cash = brkr.get_cash_balance();

        brkr.check();
        let res = brkr.send_order(Order::market(OrderType::MarketSell, "ABC", 295.0));
        assert!(matches!(res, BrokerEvent::OrderSentToExchange(..)));

        brkr.check();
        let cash0 = brkr.get_cash_balance();

        let qty = brkr.get_position_qty("ABC").unwrap();
        assert_eq!(**qty, 200.0);
        assert!(*cash0 > *cash);
    }

    #[test]
    fn test_that_valuation_updates_in_next_period() {
        let mut brkr = setup();
        brkr.deposit_cash(&100_000.0);
        brkr.send_order(Order::market(OrderType::MarketBuy, "ABC", 495.0));
        brkr.check();

        let val = brkr.get_position_value("ABC").unwrap();

        brkr.check();
        let val1 = brkr.get_position_value("ABC").unwrap();
        assert_ne!(val, val1);
    }

    #[test]
    fn test_that_profit_calculation_is_accurate() {
        let mut brkr = setup();
        brkr.deposit_cash(&100_000.0);
        brkr.send_order(Order::market(OrderType::MarketBuy, "ABC", 495.0));
        brkr.check();

        brkr.check();

        let profit = brkr.get_position_profit("ABC").unwrap();
        assert_eq!(*profit, -4950.00);
    }

    #[test]
    fn test_that_dividends_are_paid() {
        let mut brkr = setup();
        brkr.deposit_cash(&101_000.0);
        brkr.send_order(Order::market(OrderType::MarketBuy, "ABC", 495.0));

        brkr.check();
        let cash_before_dividend = brkr.get_cash_balance();

        brkr.check();
        brkr.check();
        let cash_after_dividend = brkr.get_cash_balance();
        assert_ne!(cash_before_dividend, cash_after_dividend);
    }

    #[test]
    fn test_that_broker_build_passes_without_trade_costs() {
        let mut prices: HashMap<DateTime, Vec<Arc<Quote>>> = HashMap::new();

        let quote = Arc::new(Quote::new(100.00, 101.00, 100, "ABC"));
        let quote2 = Arc::new(Quote::new(104.00, 105.00, 101, "ABC"));
        let quote4 = Arc::new(Quote::new(95.00, 96.00, 102, "ABC"));
        prices.insert(100.into(), vec![quote]);
        prices.insert(101.into(), vec![quote2]);
        prices.insert(102.into(), vec![quote4]);

        let clock = crate::clock::ClockBuilder::with_length_in_dates(100, 102)
            .with_frequency(&crate::types::Frequency::Second)
            .build();

        let source = HashMapInputBuilder::new()
            .with_quotes(prices)
            .with_clock(clock.clone())
            .build();

        let exchange = SingleExchangeBuilder::new()
            .with_clock(clock.clone())
            .with_data_source(source.clone())
            .build();

        let _brkr = SingleBrokerBuilder::new()
            .with_data(source)
            .with_exchange(exchange)
            .with_trade_costs(vec![BrokerCost::PctOfValue(0.01)])
            .build();
    }

    #[test]
    fn test_that_broker_uses_last_value_if_it_fails_to_find_quote() {
        //If the broker cannot find a quote in the current period for a stock, it automatically
        //uses a value of zero. This is a problem because the current time could a weekend or
        //bank holiday, and if the broker is attempting to value the portfolio on that day
        //they will ask for a quote, not find one, and then use a value of zero which is
        //incorrect.

        let mut prices: HashMap<DateTime, Vec<Arc<Quote>>> = HashMap::new();
        let dividends: HashMap<DateTime, Vec<Arc<Dividend>>> = HashMap::new();
        let quote = Arc::new(Quote::new(100.00, 101.00, 100, "ABC"));
        let quote1 = Arc::new(Quote::new(10.00, 11.00, 100, "BCD"));

        let quote2 = Arc::new(Quote::new(100.00, 101.00, 101, "ABC"));
        let quote3 = Arc::new(Quote::new(10.00, 11.00, 101, "BCD"));

        let quote4 = Arc::new(Quote::new(104.00, 105.00, 102, "ABC"));

        let quote5 = Arc::new(Quote::new(104.00, 105.00, 103, "ABC"));
        let quote6 = Arc::new(Quote::new(12.00, 13.00, 103, "BCD"));

        prices.insert(100.into(), vec![quote, quote1]);
        //Trades execute here
        prices.insert(101.into(), vec![quote2, quote3]);
        //We are missing a quote for BCD on 101, but the broker should return the last seen value
        prices.insert(102.into(), vec![quote4]);
        //And when we check the next date, it updates correctly
        prices.insert(103.into(), vec![quote5, quote6]);

        let clock = crate::clock::ClockBuilder::with_length_in_seconds(100, 5)
            .with_frequency(&crate::types::Frequency::Second)
            .build();

        let source = HashMapInputBuilder::new()
            .with_quotes(prices)
            .with_dividends(dividends)
            .with_clock(clock.clone())
            .build();

        let exchange = SingleExchangeBuilder::new()
            .with_clock(clock.clone())
            .with_data_source(source.clone())
            .build();

        let mut brkr = SingleBrokerBuilder::new()
            .with_data(source)
            .with_exchange(exchange)
            .with_trade_costs(vec![BrokerCost::PctOfValue(0.01)])
            .build();

        brkr.deposit_cash(&100_000.0);
        brkr.send_order(Order::market(OrderType::MarketBuy, "ABC", 100.0));
        brkr.send_order(Order::market(OrderType::MarketBuy, "BCD", 100.0));

        brkr.check();

        //Missing live quote for BCD
        brkr.check();
        let value = brkr.get_position_value("BCD").unwrap();
        println!("{:?}", value);
        //We test against the bid price, which gives us the value exclusive of the price paid at ask
        assert!(*value == 10.0 * 100.0);

        //BCD has quote again
        brkr.check();

        let value1 = brkr.get_position_value("BCD").unwrap();
        println!("{:?}", value1);
        assert!(*value1 == 12.0 * 100.0);
    }

    #[test]
    fn test_that_broker_handles_negative_cash_balance_due_to_volatility() {
        //Because orders sent to the exchange are not executed instantaneously it is possible for a
        //broker to issue an order for a stock, the price to fall/rise before the trade gets
        //executed, and the broker end up with more/less cash than expected.
        //
        //For example, if orders are issued for 100% of the portfolio then if prices rises then we
        //can end up with negative balances.

        let mut prices: HashMap<DateTime, Vec<Arc<Quote>>> = HashMap::new();
        let quote = Arc::new(Quote::new(100.00, 101.00, 100, "ABC"));
        let quote1 = Arc::new(Quote::new(150.00, 151.00, 101, "ABC"));
        let quote2 = Arc::new(Quote::new(150.00, 151.00, 102, "ABC"));

        prices.insert(100.into(), vec![quote]);
        prices.insert(101.into(), vec![quote1]);
        prices.insert(102.into(), vec![quote2]);

        let clock = crate::clock::ClockBuilder::with_length_in_seconds(100, 5)
            .with_frequency(&crate::types::Frequency::Second)
            .build();

        let source = HashMapInputBuilder::new()
            .with_quotes(prices)
            .with_clock(clock.clone())
            .build();

        let exchange = SingleExchangeBuilder::new()
            .with_clock(clock.clone())
            .with_data_source(source.clone())
            .build();

        let mut brkr = SingleBrokerBuilder::new()
            .with_data(source)
            .with_exchange(exchange)
            .with_trade_costs(vec![BrokerCost::PctOfValue(0.01)])
            .build();

        brkr.deposit_cash(&100_000.0);
        //Because the price of ABC rises after this order is sent, we will end up with a negative
        //cash balance after the order is executed
        brkr.send_order(Order::market(OrderType::MarketBuy, "ABC", 700.0));

        //Trades execute
        brkr.check();

        let cash = brkr.get_cash_balance();
        assert!(*cash < 0.0);

        //Broker rebalances to raise cash
        brkr.check();
        let cash1 = brkr.get_cash_balance();
        assert!(*cash1 > 0.0);
    }
}