//! Health endpoints.
//!
//! Two distinct questions, deliberately not merged:
//!
//! * `/health/live`  - is the process running? Never touches a dependency, so a
//!   database outage does not make an orchestrator kill an otherwise fine
//!   process.
//! * `/health/ready` - can it serve traffic? Checks every dependency and
//!   returns 503 if any required one is down.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use phonix_web::state::AppState;
use serde::Serialize;

#[derive(Serialize)]
pub struct Liveness {
    status: &'static str,
    version: &'static str,
}

pub async fn liveness() -> Json<Liveness> {
    Json(Liveness {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Serialize)]
pub struct Readiness {
    status: &'static str,
    environment: String,
    checks: Vec<Check>,
    live_tenant_pools: u64,
}

#[derive(Serialize)]
pub struct Check {
    name: &'static str,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

impl Check {
    fn ok(name: &'static str) -> Self {
        Self {
            name,
            status: "ok",
            detail: None,
        }
    }

    fn skipped(name: &'static str) -> Self {
        Self {
            name,
            status: "skipped",
            detail: Some("disabled in configuration".to_owned()),
        }
    }

    fn failed(name: &'static str, detail: impl ToString) -> Self {
        Self {
            name,
            status: "failed",
            detail: Some(detail.to_string()),
        }
    }

    fn is_failed(&self) -> bool {
        self.status == "failed"
    }
}

pub async fn readiness(State(state): State<AppState>) -> (StatusCode, Json<Readiness>) {
    let mut checks = Vec::with_capacity(3);

    // Catalog database: required. Without it no tenant can be routed at all.
    checks.push(
        match phonix_db::sqlx::query("SELECT 1")
            .execute(state.catalog.pool())
            .await
        {
            Ok(_) => Check::ok("postgres.catalog"),
            Err(err) => Check::failed("postgres.catalog", err),
        },
    );

    // Redis: required only when enabled. With `fail_open` the app survives a
    // cache outage, but readiness should still report it honestly.
    checks.push(if state.cache.is_enabled() {
        match state.cache.ping().await {
            Ok(()) => Check::ok("redis"),
            Err(err) => Check::failed("redis", err),
        }
    } else {
        Check::skipped("redis")
    });

    checks.push(match &state.publisher {
        Some(_) => Check::ok("rabbitmq"),
        None => Check::skipped("rabbitmq"),
    });

    let healthy = !checks.iter().any(Check::is_failed);

    let body = Readiness {
        status: if healthy { "ready" } else { "degraded" },
        environment: state.config.app.environment.clone(),
        checks,
        live_tenant_pools: state.tenants.live_pools(),
    };

    let code = if healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (code, Json(body))
}
