use std::collections::HashMap;
use std::marker::PhantomData;

use log::info;

use crate::broker::{
    BrokerCashEvent, BrokerOperations, BrokerOrder, BrokerQuote, BrokerStates, CashOperations,
    Clock, Portfolio, SendOrder, StrategySnapshot, Update,
};
use crate::perf::{BacktestOutput, PerformanceCalculator};
use crate::schedule::{DefaultTradingSchedule, TradingSchedule};
use crate::strategy::StrategyEvent;

pub trait StaticWeightBroker<Q: BrokerQuote, O: BrokerOrder>:
    CashOperations<Q>
    + BrokerOperations<O, Q>
    + Portfolio<Q>
    + SendOrder<O>
    + BrokerStates
    + Update
    + Clock
{
}

pub type PortfolioAllocation = HashMap<String, f64>;

pub struct StaticWeightStrategyBuilder<Q: BrokerQuote, O: BrokerOrder, B: StaticWeightBroker<Q, O>>
{
    //If missing either field, we cannot run this strategy
    brkr: Option<B>,
    weights: Option<PortfolioAllocation>,
    _quote: PhantomData<Q>,
    _order: PhantomData<O>,
}

impl<Q: BrokerQuote, O: BrokerOrder, B: StaticWeightBroker<Q, O>>
    StaticWeightStrategyBuilder<Q, O, B>
{
    pub fn default(&mut self) -> StaticWeightStrategy<Q, O, B> {
        if self.brkr.is_none() || self.weights.is_none() {
            panic!("Strategy must have broker and weights");
        }

        let brkr = self.brkr.take();
        let weights = self.weights.take();
        StaticWeightStrategy {
            brkr: brkr.unwrap(),
            target_weights: weights.unwrap(),
            net_cash_flow: 0.0,
            history: Vec::new(),
            _quote: PhantomData,
            _order: PhantomData,
        }
    }

    pub fn with_brkr(&mut self, brkr: B) -> &mut Self {
        self.brkr = Some(brkr);
        self
    }

    pub fn with_weights(&mut self, weights: PortfolioAllocation) -> &mut Self {
        self.weights = Some(weights);
        self
    }

    pub fn new() -> Self {
        Self {
            brkr: None,
            weights: None,
            _quote: PhantomData,
            _order: PhantomData,
        }
    }
}

impl<Q: BrokerQuote, O: BrokerOrder, B: StaticWeightBroker<Q, O>> Default
    for StaticWeightStrategyBuilder<Q, O, B>
{
    fn default() -> Self {
        Self::new()
    }
}

///Basic implementation of an investment strategy which takes a set of fixed-weight allocations and
///rebalances over time towards those weights.
pub struct StaticWeightStrategy<Q: BrokerQuote, O: BrokerOrder, B: StaticWeightBroker<Q, O>> {
    brkr: B,
    target_weights: PortfolioAllocation,
    net_cash_flow: f64,
    history: Vec<StrategySnapshot>,
    _quote: PhantomData<Q>,
    _order: PhantomData<O>,
}

impl<Q: BrokerQuote, O: BrokerOrder, B: StaticWeightBroker<Q, O>> StaticWeightStrategy<Q, O, B> {
    pub async fn run(&mut self) {
        while self.brkr.has_next() {
            self.update().await;
        }
    }

    pub fn perf(&self, freq: crate::perf::Frequency) -> BacktestOutput {
        //Intended to be called at end of simulation
        let hist = self.get_history();
        PerformanceCalculator::calculate(freq, hist)
    }

    pub fn get_snapshot(&mut self) -> StrategySnapshot {
        // Defaults to zero inflation because most users probably aren't looking
        // for real returns calcs
        let now = self.brkr.now();
        StrategySnapshot {
            date: now.into(),
            portfolio_value: self.brkr.get_total_value(),
            net_cash_flow: self.net_cash_flow,
            inflation: 0.0,
        }
    }

    pub fn init(&mut self, initital_cash: &f64) {
        self.deposit_cash(initital_cash);
        if DefaultTradingSchedule::should_trade(&self.brkr.now().into()) {
            let orders = self
                .brkr
                .diff_brkr_against_target_weights(&self.target_weights);
            if !orders.is_empty() {
                self.brkr.send_orders(&orders);
            }
        }
    }

    pub async fn update(&mut self) {
        self.brkr.check().await;
        let now = self.brkr.now();
        if DefaultTradingSchedule::should_trade(&now.into()) {
            let orders = self
                .brkr
                .diff_brkr_against_target_weights(&self.target_weights);
            if !orders.is_empty() {
                self.brkr.send_orders(&orders);
            }
        }
        let snap = self.get_snapshot();
        self.history.push(snap);
    }

    fn deposit_cash(&mut self, cash: &f64) -> StrategyEvent {
        info!("STRATEGY: Depositing {:?} into strategy", cash);
        self.brkr.deposit_cash(cash);
        self.net_cash_flow += self.net_cash_flow;
        StrategyEvent::DepositSuccess(*cash)
    }

    pub fn withdraw_cash(&mut self, cash: &f64) -> StrategyEvent {
        if let BrokerCashEvent::WithdrawSuccess(withdrawn) = self.brkr.withdraw_cash(cash) {
            info!("STRATEGY: Succesfully withdrew {:?} from strategy", cash);
            self.net_cash_flow -= withdrawn;
            return StrategyEvent::WithdrawSuccess(*cash);
        }
        info!("STRATEGY: Failed to withdraw {:?} from strategy", cash);
        StrategyEvent::WithdrawFailure(*cash)
    }

    pub fn withdraw_cash_with_liquidation(&mut self, cash: &f64) -> StrategyEvent {
        if let BrokerCashEvent::WithdrawSuccess(withdrawn) =
            //No logging here because the implementation is fully logged due to the greater
            //complexity of this task vs standard withdraw
            self.brkr.withdraw_cash_with_liquidation(cash)
        {
            self.net_cash_flow -= withdrawn;
            return StrategyEvent::WithdrawSuccess(*cash);
        }
        StrategyEvent::WithdrawFailure(*cash)
    }

    pub fn get_history(&self) -> Vec<StrategySnapshot> {
        self.history.clone()
    }
}
