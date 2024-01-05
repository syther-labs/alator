use std::sync::Mutex;

use actix_web::web;
use serde::{Deserialize, Serialize};

use crate::exchange::uist::{UistOrder, UistOrderId, UistQuote, UistTrade, UistV1};

pub struct AppState {
    pub exchange: Mutex<UistV1>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TickResponse {
    pub has_next: bool,
    pub executed_trades: Vec<UistTrade>,
    pub inserted_orders: Vec<UistOrder>,
}

pub async fn tick(app: web::Data<AppState>) -> web::Json<TickResponse> {
    let mut ex = app.exchange.lock().unwrap();

    let tick = ex.tick();
    web::Json(TickResponse {
        inserted_orders: tick.2,
        executed_trades: tick.1,
        has_next: tick.0,
    })
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeleteOrderRequest {
    pub order_id: UistOrderId,
}

pub async fn delete_order(
    app: web::Data<AppState>,
    delete_order: web::Json<DeleteOrderRequest>,
) -> web::Json<()> {
    let mut ex = app.exchange.lock().unwrap();
    ex.delete_order(delete_order.order_id);
    web::Json(())
}

#[derive(Debug, Deserialize, Serialize)]
pub struct InsertOrderRequest {
    pub order: UistOrder,
}

pub async fn insert_order(
    app: web::Data<AppState>,
    insert_order: web::Json<InsertOrderRequest>,
) -> web::Json<()> {
    dbg!(&insert_order);
    let mut ex = app.exchange.lock().unwrap();
    ex.insert_order(insert_order.order.clone());
    web::Json(())
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FetchTradesRequest {
    pub from: UistOrderId,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FetchQuotesResponse {
    pub quotes: Vec<UistQuote>,
}

pub async fn fetch_quotes(app: web::Data<AppState>) -> web::Json<FetchQuotesResponse> {
    let ex = app.exchange.lock().unwrap();
    web::Json(FetchQuotesResponse {
        quotes: ex.fetch_quotes(),
    })
}

#[derive(Debug, Deserialize, Serialize)]
pub struct InitResponse {
    pub start: i64,
    pub frequency: u8,
}

pub async fn init(app: web::Data<AppState>) -> web::Json<InitResponse> {
    let ex = app.exchange.lock().unwrap();
    let init = ex.init();
    web::Json(InitResponse {
        start: init.start,
        frequency: init.frequency,
    })
}

#[derive(Debug, Deserialize, Serialize)]
pub struct InfoResponse {
    pub version: String,
    pub dataset: String,
}

pub async fn info(app: web::Data<AppState>) -> web::Json<InfoResponse> {
    let ex = app.exchange.lock().unwrap();
    let info = ex.info();
    web::Json(InfoResponse {
        version: info.version,
        dataset: info.dataset,
    })
}

#[cfg(test)]
mod tests {
    use actix_web::{test, App};

    use crate::exchange::uist::random_uist_generator;

    use super::*;

    #[actix_web::test]
    async fn test_single_trade_loop() {
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(Mutex::new(random_uist_generator(3000).0)))
                .route("/", web::get().to(info))
                .route("/init", web::get().to(init))
                .route("/fetch_quotes", web::get().to(fetch_quotes))
                .route("/tick", web::get().to(tick))
                .route("/insert_order", web::post().to(insert_order))
                .route("/delete_order", web::post().to(delete_order)),
        )
        .await;

        let req = test::TestRequest::get().uri("/init").to_request();
        let resp: InitResponse = test::call_and_read_body_json(&app, req).await;
        assert!(resp.frequency == 0);

        let req1 = test::TestRequest::get().uri("/fetch_quotes").to_request();
        let _resp1: FetchQuotesResponse = test::call_and_read_body_json(&app, req1).await;

        let req2 = test::TestRequest::get().uri("/tick").to_request();
        let _resp2: TickResponse = test::call_and_read_body_json(&app, req2).await;

        let req3 = test::TestRequest::post()
            .set_json(InsertOrderRequest {
                order: UistOrder::market_buy("ABC", 100.0),
            })
            .uri("/insert_order")
            .to_request();
        test::call_and_read_body(&app, req3).await;

        let req4 = test::TestRequest::get().uri("/tick").to_request();
        let resp4: TickResponse = test::call_and_read_body_json(&app, req4).await;

        assert!(resp4.executed_trades.len() == 1);
        assert!(resp4.executed_trades.get(0).unwrap().symbol == "ABC")
    }
}