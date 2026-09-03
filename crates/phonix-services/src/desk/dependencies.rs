//! Whether the things this deployment depends on can be reached from this box.
//!
//! Three answers, the same three `phonix-server`'s `/health/ready` gives -
//! `postgres.catalog`, `redis`, `rabbitmq`, and there is no storage check
//! because there is no storage dependency to check. ADR 0005 section 6 lists
//! this as one of the questions Desk exists to answer, and it is the question
//! asked first when a workspace is behaving strangely: is this the application,
//! or is this the box.
//!
//! # Two of the three are reachability only, on purpose
//!
//! The catalog is asked `SELECT 1` over the pool Desk already holds, which is
//! a real answer from Postgres. Redis and RabbitMQ are asked only whether the
//! configured port accepts a connection - Desk opens a socket, sees that it
//! opened, and closes it.
//!
//! That is a weaker check than the server's, and deliberately so. `phonix-desk`
//! depends on neither client crate: pulling in `redis` for a `PING` and `lapin`
//! for an AMQP handshake would put the product's infrastructure stack inside
//! the tool whose whole value is being simpler than what it watches, and
//! `Messaging::connect` declares the exchange topology as a side effect, so a
//! health check written on it would not be a health check - it would be Desk
//! writing to the broker. A page that says "the port answers" and means it is
//! worth more than one that says "healthy" and had to grow a dependency graph
//! to say it.
//!
//! What it costs: each probe is a TCP connection opened and dropped without a
//! protocol handshake, so RabbitMQ logs a client that closed unexpectedly once
//! per page load. That is a known and accepted line in somebody else's log.
//!
//! # A probe cannot fail
//!
//! [`probe`] returns a [`Report`] and not a `Result`. Reporting a failure is
//! this module's entire job, so a failure it could return instead of describe
//! would be a hole in the only thing it does.

use std::time::{Duration, Instant};

use phonix_config::{DatabaseConfig, RabbitMqConfig, RedisConfig};
use phonix_db::tenancy::catalog::Catalog;
use tokio::net::TcpStream;

/// How long any one probe may take before it is reported as unreachable.
///
/// Deliberately not each service's own `connect_timeout_secs`: those are tuned
/// for a request that has to succeed and can afford to wait. This is a page
/// that has to answer, and it is opened at the moment something is already
/// wrong. A dependency that has not answered in three seconds is a dependency
/// somebody needs to be told about, so the page says so rather than hanging on
/// it.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// What a probe found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Reachable,
    Unreachable,
    /// Switched off in configuration. Neither a pass nor a failure, and shown
    /// as its own word: a Redis nobody configured is not a Redis that is down,
    /// and the two must never read the same on a page somebody is scanning.
    Disabled,
}

impl Standing {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reachable => "ok",
            Self::Unreachable => "unreachable",
            Self::Disabled => "off",
        }
    }

    pub fn is_unreachable(self) -> bool {
        matches!(self, Self::Unreachable)
    }
}

/// One dependency, and what asking it cost.
pub struct Check {
    /// The name `/health/ready` already uses for it, so the two surfaces can be
    /// compared without translating.
    pub name: &'static str,
    /// Host and port, as configured. Shown because "redis is down" and "redis
    /// is up and this box is pointed at the wrong one" are different problems
    /// with the same symptom.
    pub target: String,
    /// How deeply it was asked. Lives on the check rather than in the page's
    /// prose so that a check whose depth changes cannot leave the sentence
    /// describing it behind.
    pub method: &'static str,
    pub standing: Standing,
    /// Why, when there is a why. The connection error, or the word that a
    /// disabled dependency was never asked.
    pub detail: Option<String>,
    pub took: Duration,
}

impl Check {
    fn disabled(name: &'static str, target: String) -> Self {
        Self {
            name,
            target,
            method: "not asked",
            standing: Standing::Disabled,
            detail: Some("disabled in configuration".to_owned()),
            took: Duration::ZERO,
        }
    }
}

/// Every dependency, in the order they matter.
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    /// How many are down. Drives the one sentence at the top of the page, and
    /// is not the same as "how many are not ok" - a disabled dependency is not
    /// a problem to be counted.
    pub fn unreachable(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| check.standing.is_unreachable())
            .count()
    }

    pub fn all_well(&self) -> bool {
        self.unreachable() == 0
    }
}

/// Ask all three, at once.
///
/// Concurrently rather than in turn: sequentially, three dependencies that are
/// all down would take three times [`PROBE_TIMEOUT`] to say so, and the page
/// would be slowest in exactly the case it is most needed.
pub async fn probe(
    catalog: &Catalog,
    database: &DatabaseConfig,
    redis: &RedisConfig,
    rabbit: &RabbitMqConfig,
) -> Report {
    let (catalog, redis, rabbit) = tokio::join!(
        catalog_check(catalog, database),
        port_check("redis", &redis.host, redis.port, redis.enabled),
        port_check("rabbitmq", &rabbit.host, rabbit.port, rabbit.enabled),
    );

    Report {
        checks: vec![catalog, redis, rabbit],
    }
}

/// The one dependency Desk holds a connection to, so the one it can ask a real
/// question of.
async fn catalog_check(catalog: &Catalog, database: &DatabaseConfig) -> Check {
    let target = format!("{}:{}", database.host, database.port);
    let started = Instant::now();

    // Through the pool Desk already has, which is the honest test: a pool with
    // no free connection is a catalog this process cannot use, however healthy
    // Postgres itself may be.
    let outcome = tokio::time::timeout(
        PROBE_TIMEOUT,
        sqlx::query("SELECT 1").execute(catalog.pool()),
    )
    .await;

    let (standing, detail) = match outcome {
        Ok(Ok(_)) => (Standing::Reachable, None),
        Ok(Err(err)) => (Standing::Unreachable, Some(err.to_string())),
        Err(_) => (Standing::Unreachable, Some(timed_out())),
    };

    Check {
        name: "postgres.catalog",
        target,
        method: "SELECT 1",
        standing,
        detail,
        took: started.elapsed(),
    }
}

/// Does the configured port accept a connection?
///
/// The socket is dropped as soon as it opens. Nothing is sent, nothing is read,
/// and no protocol handshake is attempted - see this module's header for why
/// that is the check rather than a shortcut to one.
async fn port_check(name: &'static str, host: &str, port: u16, enabled: bool) -> Check {
    let target = format!("{host}:{port}");

    if !enabled {
        return Check::disabled(name, target);
    }

    let started = Instant::now();
    let outcome = tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(target.clone())).await;

    let (standing, detail) = match outcome {
        Ok(Ok(_stream)) => (Standing::Reachable, None),
        Ok(Err(err)) => (Standing::Unreachable, Some(err.to_string())),
        Err(_) => (Standing::Unreachable, Some(timed_out())),
    };

    Check {
        name,
        target,
        method: "TCP connect",
        standing,
        detail,
        took: started.elapsed(),
    }
}

/// One sentence for a timeout, in seconds rather than in a `Duration`'s Debug.
fn timed_out() -> String {
    format!("did not answer within {}s", PROBE_TIMEOUT.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &'static str, standing: Standing) -> Check {
        Check {
            name,
            target: "host:1".to_owned(),
            method: "TCP connect",
            standing,
            detail: None,
            took: Duration::ZERO,
        }
    }

    /// The distinction the page is drawn around: nobody configured RabbitMQ is
    /// not the same news as RabbitMQ is down, so a disabled check must not
    /// raise the alarm at the top of the page.
    #[test]
    fn a_disabled_dependency_is_not_counted_as_a_problem() {
        let report = Report {
            checks: vec![
                check("postgres.catalog", Standing::Reachable),
                check("redis", Standing::Disabled),
                check("rabbitmq", Standing::Disabled),
            ],
        };

        assert_eq!(report.unreachable(), 0);
        assert!(report.all_well());
    }

    #[test]
    fn one_dependency_down_is_counted_and_says_so() {
        let report = Report {
            checks: vec![
                check("postgres.catalog", Standing::Reachable),
                check("redis", Standing::Unreachable),
                check("rabbitmq", Standing::Disabled),
            ],
        };

        assert_eq!(report.unreachable(), 1);
        assert!(!report.all_well());
    }

    /// A port nothing is listening on is refused rather than left hanging, so
    /// this is the fast path and needs no timeout to be a fair test.
    #[tokio::test]
    async fn a_closed_port_is_reported_unreachable_with_the_reason() {
        // Port 1 on loopback: reserved, and nothing in this workspace binds it.
        let check = port_check("redis", "127.0.0.1", 1, true).await;

        assert_eq!(check.standing, Standing::Unreachable);
        assert_eq!(check.target, "127.0.0.1:1");
        assert!(check.detail.is_some(), "a failure has to say why");
    }

    /// A dependency switched off is never dialled, which is what makes the
    /// panel usable on a box where Redis was never installed.
    #[tokio::test]
    async fn a_disabled_dependency_is_not_dialled_at_all() {
        let check = port_check("rabbitmq", "203.0.113.1", 5672, false).await;

        assert_eq!(check.standing, Standing::Disabled);
        assert_eq!(check.method, "not asked");
        assert_eq!(check.took, Duration::ZERO);
    }

    /// The one thing that must be reachable is listening, so this proves the
    /// probe reports a success and not only a failure.
    #[tokio::test]
    async fn a_port_that_is_listening_is_reported_reachable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let check = port_check("redis", "127.0.0.1", port, true).await;

        assert_eq!(check.standing, Standing::Reachable);
        assert!(check.detail.is_none(), "a pass explains nothing");
    }
}
